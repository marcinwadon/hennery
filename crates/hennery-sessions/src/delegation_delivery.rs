//! Durable reverse delivery. Source facts and notices commit together; only
//! matching turn facts acknowledge delivery. All content is read from the log.
use anyhow::Result;
use hennery_proto::{plugin::InputOrigin, rest::EventDto};
use rusqlite::{OptionalExtension, Transaction, params};

/// Called for applied host facts and collector facts in their own transaction.
pub(crate) fn record_in(tx: &Transaction<'_>, owner: &str, event: &EventDto) -> Result<()> {
    let closing: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2
        AND (close_requested=1 OR lifecycle IN ('closed','archived','deleted')))",
        params![owner, event.session_id],
        |r| r.get(0),
    )?;
    if closing {
        return Ok(());
    }
    let pending = event.body.get("pending_id").and_then(serde_json::Value::as_str);
    let reason = event.body.get("reason").and_then(serde_json::Value::as_str);
    let turn = event.body.get("turn_id").and_then(serde_json::Value::as_str);
    let loss = matches!(event.kind.as_str(), "adapter_exited" | "host_restarted")
        || event.kind == "presumed_parked" && reason == Some("host_revoked")
        || event.kind == "session_parked" && reason == Some("adapter_exited")
        || event.kind == "turn_ended_synthesized"
        || event.kind == "turn_ended" && event.body["outcome"] == "interrupted"
        || matches!(event.kind.as_str(), "pending_cancelled" | "pending_resolved")
            && matches!(
                reason,
                Some("adapter_lost" | "host_restarted" | "host_revoked" | "session_parked")
            );
    let kind = if loss {
        "session_unavailable"
    } else {
        match event.kind.as_str() {
            "answer_submitted" => {
                obsolete_in(tx, owner, &event.session_id, pending)?;
                return Ok(());
            }
            "turn_ended" => "turn_ended",
            "pending_opened" => "question",
            "pending_resolved" | "pending_cancelled" => {
                obsolete_in(tx, owner, &event.session_id, pending)?;
                if event.body["resolution"] == "delivered" {
                    return Ok(());
                }
                "question_cancelled"
            }
            "start_failed" | "start_not_delivered" => "session_failed",
            _ => return Ok(()),
        }
    };
    if loss {
        obsolete_in(tx, owner, &event.session_id, None)?;
    }
    let family: Option<(String, String, String, i64, bool, String)> = tx
        .query_row(
            "SELECT f.parent_id,f.root_id,f.hat_id,g.revision,
         f.detached = 0 AND p.close_requested = 0 AND p.lifecycle IN ('starting','active','parked')
         AND g.delegation_enabled = 1 AND p.hat_id = f.hat_id AND c.hat_id = f.hat_id,
         json_object('model',p.model,'mode',p.mode,'axes',json(COALESCE(p.config_axes,json_object())))
         FROM session_families f JOIN sessions p ON p.owner_id = ?1 AND p.id = f.parent_id
         JOIN sessions c ON c.owner_id = ?1 AND c.id = f.child_id
         JOIN hat_session_tools g ON g.owner_id = ?1 AND g.hat_id = f.hat_id
         WHERE f.owner_id = ?1 AND f.child_id = ?2",
            params![owner, event.session_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    if family.is_none() && !delegated_in(tx, owner, &event.session_id)? {
        return Ok(());
    }
    let attach:i64 = tx.query_row("SELECT COALESCE(MAX(event_id),0) FROM events WHERE owner_id = ?1 AND session_id = ?2 AND kind = 'session_started' AND applied = 1", params![owner,event.session_id],|r|r.get(0))?;
    let incident = if loss {
        format!("loss:{attach}")
    } else if kind == "session_failed" {
        let generation:i64=tx.query_row("SELECT COALESCE((SELECT attach_generation FROM session_attach_evidence WHERE owner_id = ?1 AND session_id = ?2),0)",params![owner,event.session_id],|r|r.get(0))?;
        format!("failure:{attach}:{generation}")
    } else {
        format!("{}:{}", event.event_id, kind)
    };
    let Some((parent, root, hat, revision, eligible, required)) = family else {
        crate::delegation_attention::record_orphan_in(tx, owner, event, kind, &incident, pending)?;
        return Ok(());
    };
    let parent_closed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id=?2
        AND (close_requested=1 OR lifecycle IN ('closed','archived','deleted')))",
        params![owner, parent],
        |r| r.get(0),
    )?;
    if parent_closed && !crate::delegation_attention::child_needs_human(tx, owner, &event.session_id)? {
        return Ok(());
    }
    let permission = kind == "question" && tx.query_row("SELECT EXISTS(SELECT 1 FROM pending WHERE owner_id = ?1 AND session_id = ?2 AND pending_id = ?3 AND kind = 'permission')",params![owner,event.session_id,pending],|r|r.get::<_,bool>(0))?;
    let queued:i64 = tx.query_row("SELECT COUNT(*) FROM delegation_notices WHERE owner_id = ?1 AND parent_id = ?2 AND state IN ('parent_queued','waiting_config')",params![owner,parent],|r|r.get(0))?;
    let disposition = if parent_closed {
        Some("parent_closed_child_needs_human")
    } else if !eligible {
        Some("parent_unavailable")
    } else if permission {
        Some("operator_required")
    } else if queued >= 32 {
        Some("queue_full")
    } else {
        None
    };
    let now = hennery_kernel::secret::unix_now();
    tx.execute("INSERT INTO delegation_notices(owner_id,delivery_id,child_id,parent_id,root_id,hat_id,
        source_event_id,source_events,kind,source_turn,pending_id,incident,accepted_revision,created_at,deadline,state,reason,required_config)
        VALUES (?1,?2,?3,?4,?5,?6,?7,json_array(?7),?8,?9,?10,?11,?12,?13,?13+300,?14,?15,?16)
        ON CONFLICT(owner_id,child_id,incident) DO UPDATE SET
        source_events = CASE WHEN instr(',' || trim(delegation_notices.source_events,'[]') || ',', ',' || CAST(?7 AS TEXT) || ',') = 0
            THEN json_insert(delegation_notices.source_events,'$[#]',?7) ELSE delegation_notices.source_events END,
        source_turn = COALESCE(delegation_notices.source_turn,excluded.source_turn)
        WHERE delegation_notices.owner_id = ?1 AND json_array_length(delegation_notices.source_events) < 128",
        params![owner,uuid::Uuid::now_v7().to_string(),event.session_id,parent,root,hat,event.event_id,kind,turn,pending,incident,revision,now,
        if disposition.is_some(){"operator_required"}else{"parent_queued"},disposition,required])?;
    // Reserve attention in the source transaction. Lifecycle/grant triggers
    // activate this row using UPDATE, preserving the existing SQL owner audit.
    tx.execute("INSERT INTO delegation_attention(owner_id,attention_id,delivery_id,child_id,hat_id,reason,pending_id,state,operator_epoch,incident)
        SELECT ?1,n.delivery_id,n.delivery_id,n.child_id,c.hat_id,COALESCE(n.reason,'operator_required'),
        CASE WHEN n.kind = 'question' THEN n.pending_id END,
        CASE WHEN n.state = 'operator_required' THEN 'open' ELSE 'standby' END,
        COALESCE((SELECT MAX(event_id) FROM events WHERE owner_id = ?1 AND session_id = n.root_id
            AND kind = 'user_turn' AND applied = 1 AND COALESCE(json_extract(body,'$.origin.kind'),'operator') = 'operator'),0),n.incident
        FROM delegation_notices n JOIN sessions c ON c.owner_id = ?1 AND c.id = n.child_id
        WHERE n.owner_id = ?1 AND n.child_id = ?2 AND n.incident = ?3
        ON CONFLICT DO NOTHING",params![owner,event.session_id,incident])?;
    Ok(())
}

fn obsolete_in(tx: &Transaction<'_>, owner: &str, child: &str, pending: Option<&str>) -> Result<()> {
    tx.execute(
        "UPDATE delegation_notices SET state = 'obsolete',reason = 'question_unavailable'
        WHERE owner_id = ?1 AND child_id = ?2 AND kind = 'question' AND (?3 IS NULL OR pending_id = ?3)
        AND state IN ('parent_queued','waiting_config','operator_required','parent_delivered')",
        params![owner, child, pending],
    )?;
    tx.execute(
        "UPDATE delegation_attention SET state = 'resolved' WHERE owner_id = ?1 AND child_id = ?2
        AND pending_id IS NOT NULL AND (?3 IS NULL OR pending_id = ?3)",
        params![owner, child, pending],
    )?;
    Ok(())
}

impl crate::store::Store {
    pub(crate) fn record_delegation_failure_in(&self, tx: &Transaction<'_>, session: &str, code: &str) -> Result<()> {
        let child = delegated_in(tx, self.owner_id(), session)?;
        if child {
            crate::store::collector_event(
                tx,
                self.owner_id(),
                session,
                "start_failed",
                serde_json::json!({"code":code}),
                &crate::store::now(),
            )?;
        }
        Ok(())
    }
    /// The destination's origin is written by the internal dispatcher in the
    /// reservation transaction. Public prompts cannot supply delegated origins,
    /// and host facts copy this value rather than choosing one. Compare strict
    /// typed bytes from the surviving turn: deleting a source scrubs its routing
    /// receipts without invalidating already delivered historical provenance.
    /// This evidence grants no current family or session-control authority.
    pub(crate) fn verified_delegated_input_in(
        &self,
        tx: &Transaction<'_>,
        origin: &InputOrigin,
        target: &str,
        turn: &str,
    ) -> Result<bool> {
        if !matches!(origin, InputOrigin::ChildReport { .. } | InputOrigin::Parent { .. }) {
            return Ok(false);
        }
        let saved: Option<String> = tx
            .query_row(
                "SELECT input_origin FROM turns WHERE owner_id = ?1 AND session_id = ?2 AND turn_id = ?3",
                params![self.owner_id(), target, turn],
                |r| r.get(0),
            )
            .optional()?;
        Ok(saved
            .and_then(|raw| serde_json::from_str::<InputOrigin>(&raw).ok())
            .as_ref()
            == Some(origin))
    }
}

fn delegated_in(tx: &Transaction<'_>, owner: &str, session: &str) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE owner_id = ?1 AND id = ?2 AND delegated_child = 1)",
        params![owner, session],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_report_provenance_requires_every_committed_delivery_reference() {
        let store = crate::store::Store::open_in_memory().unwrap();
        for id in ["parent", "child"] {
            store
                .create_session(id, "host", "fake", "/project", "hat", None)
                .unwrap();
        }
        let mut conn = store.conn();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "INSERT INTO turns(owner_id,turn_id,session_id,content,created_at,input_origin)
            VALUES (?1,'turn','parent','[]','test',?2)",
            params![
                store.owner_id(),
                serde_json::to_string(&InputOrigin::ChildReport {
                    child_id: "child".into(),
                    source_event_id: "7".into(),
                    delivery_id: "delivery".into()
                })
                .unwrap()
            ],
        )
        .unwrap();
        let origin = |child: &str, event: &str, delivery: &str| InputOrigin::ChildReport {
            child_id: child.into(),
            source_event_id: event.into(),
            delivery_id: delivery.into(),
        };
        assert!(
            store
                .verified_delegated_input_in(&tx, &origin("child", "7", "delivery"), "parent", "turn")
                .unwrap()
        );
        for (child, event, delivery, parent, turn) in [
            ("other", "7", "delivery", "parent", "turn"),
            ("child", "8", "delivery", "parent", "turn"),
            ("child", "7", "other", "parent", "turn"),
            ("child", "7", "delivery", "other", "turn"),
            ("child", "7", "delivery", "parent", "other"),
        ] {
            assert!(
                !store
                    .verified_delegated_input_in(&tx, &origin(child, event, delivery), parent, turn)
                    .unwrap()
            );
        }
        let foreign = crate::store::Store::open_in_memory().unwrap();
        assert!(
            !foreign
                .verified_delegated_input_in(&tx, &origin("child", "7", "delivery"), "parent", "turn")
                .unwrap()
        );
        for raw in [
            r#"{"kind":"child_report","child_id":"child","source_event_id":"7","delivery_id":"delivery","kind":"operator"}"#,
            r#"{"kind":"operator","kind":"child_report","child_id":"child","source_event_id":"7","delivery_id":"delivery"}"#,
            r#"{"kind":"operator"}"#,
            r#"{"kind":"child_report","child_id":"child","source_event_id":"7","delivery_id":"delivery","approved":true}"#,
        ] {
            tx.execute(
                "UPDATE turns SET input_origin = ?2 WHERE owner_id = ?1 AND turn_id = 'turn'",
                params![store.owner_id(), raw],
            )
            .unwrap();
            assert!(
                !store
                    .verified_delegated_input_in(&tx, &origin("child", "7", "delivery"), "parent", "turn")
                    .unwrap()
            );
        }
    }
}
