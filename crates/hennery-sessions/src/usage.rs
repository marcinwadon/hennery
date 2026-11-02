//! Durable usage observations. No prices, cache-residency guesses or invoice inference.
use crate::store::Store;
use anyhow::Result;
use hennery_proto::usage::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

pub(crate) fn retention_in(conn: &Connection, owner: &str, id: &str) -> Result<CacheRetention> {
    let value: String = conn.query_row(
        "SELECT cache_retention FROM sessions WHERE owner_id = ?1 AND id=?2",
        params![owner, id],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_value(Value::String(value))?)
}

fn empty_totals() -> UsageTotals {
    UsageTotals {
        tokens: None,
        models: vec![],
        turns: 0,
        reported_turns: 0,
        since: None,
        cost: None,
        partial: false,
    }
}
fn add_tokens(total: &mut Option<TokenUsage>, tokens: &TokenUsage) -> bool {
    if let Some(total) = total {
        total.add(tokens)
    } else {
        *total = Some(tokens.clone());
        true
    }
}
fn add_models(total: &mut Vec<ModelUsage>, models: &[ModelUsage]) {
    for model in models {
        if let Some(row) = total.iter_mut().find(|row| row.model == model.model) {
            row.tokens.add(&model.tokens);
        } else if total.len() < 32 {
            total.push(model.clone());
        }
    }
    total.sort_by(|a, b| a.model.cmp(&b.model));
}

pub(crate) fn record_turn_in(
    conn: &Connection,
    owner: &str,
    id: &str,
    turn: &str,
    usage: &TurnUsage,
    ts: &str,
) -> Result<()> {
    if !usage.valid() {
        return Ok(());
    }
    let changed = conn.execute("UPDATE turns SET usage=?4,usage_at=?5 WHERE owner_id = ?1 AND session_id=?2 AND turn_id=?3 AND state='ended' AND usage IS NULL",
        params![owner,id,turn,serde_json::to_string(usage)?,ts])?;
    if changed == 0 {
        return Ok(());
    }
    let old: Option<String> = conn.query_row(
        "SELECT usage_totals FROM sessions WHERE owner_id = ?1 AND id=?2",
        params![owner, id],
        |r| r.get(0),
    )?;
    let mut total: UsageTotals = old
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_else(empty_totals);
    total.partial |= !add_tokens(&mut total.tokens, &usage.tokens);
    add_models(&mut total.models, &usage.models);
    total.reported_turns += 1;
    total.since.get_or_insert_with(|| ts.into());
    conn.execute(
        "UPDATE sessions SET usage_totals=?3 WHERE owner_id = ?1 AND id=?2",
        params![owner, id, serde_json::to_string(&total)?],
    )?;
    Ok(())
}

pub(crate) fn observe_context_in(
    conn: &Connection,
    owner: &str,
    id: &str,
    payload: &Value,
    early: bool,
    ts: &str,
) -> Result<()> {
    // session/load replay must not make historical observations look fresh.
    if early {
        return Ok(());
    }
    let Some(update) = payload.get("update").filter(|v| v["sessionUpdate"] == "usage_update") else {
        return Ok(());
    };
    if let Some((used, size)) = update["used"]
        .as_u64()
        .zip(update["size"].as_u64())
        .filter(|(used, size)| *used <= TokenUsage::MAX && *size > 0 && *size <= TokenUsage::MAX)
    {
        let context = ContextUsage {
            used,
            size,
            observed_at: ts.into(),
        };
        conn.execute(
            "UPDATE sessions SET usage_context=?3 WHERE owner_id = ?1 AND id=?2",
            params![owner, id, serde_json::to_string(&context)?],
        )?;
    }
    if let Some(amount) = update
        .pointer("/cost/amount")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 1e12)
        && let Some(currency) = update
            .pointer("/cost/currency")
            .and_then(Value::as_str)
            .filter(|s| s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase()))
    {
        // Retain the latest runtime total. Never sum cumulative snapshots, or
        // promise that a restarted runtime's total includes previous attachments.
        let cost = ReportedCost {
            amount,
            currency: currency.into(),
            observed_at: ts.into(),
            partial: true,
        };
        conn.execute(
            "UPDATE sessions SET usage_cost=?3 WHERE owner_id = ?1 AND id=?2",
            params![owner, id, serde_json::to_string(&cost)?],
        )?;
    }
    Ok(())
}

fn totals_in(conn: &Connection, owner: &str, id: &str) -> Result<UsageTotals> {
    let (raw, cost): (Option<String>, Option<String>) = conn.query_row(
        "SELECT usage_totals,usage_cost FROM sessions WHERE owner_id = ?1 AND id=?2",
        params![owner, id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut total = raw
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_else(empty_totals);
    total.cost = cost.map(|s| serde_json::from_str(&s)).transpose()?;
    total.turns = conn.query_row(
        "SELECT COUNT(*) FROM turns WHERE owner_id = ?1 AND session_id=?2 AND state='ended'",
        params![owner, id],
        |r| r.get::<_, i64>(0),
    )? as u64;
    total.partial |= total.tokens.is_none() || total.reported_turns < total.turns;
    Ok(total)
}

impl Store {
    pub(crate) fn requested_retention(&self, id: &str) -> Result<CacheRetention> {
        retention_in(&self.conn(), self.owner_id(), id)
    }
    pub(crate) fn initial_retention(&self, id: &str, value: CacheRetention) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET cache_retention=?3 WHERE owner_id = ?1 AND id=?2 AND lifecycle='starting'",
            params![self.owner_id(), id, serde_json::to_value(value)?.as_str()],
        )?;
        Ok(())
    }
    pub fn session_usage(&self, id: &str) -> Result<Option<SessionUsage>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
        let owner = self.owner_id();
        let row: Option<(String, String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT agent,hat_id,usage_context,cache_retention_attached FROM sessions s
             WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted'
             AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=s.hat_id)",
                params![owner, id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((source, hat, context, attached)) = row else {
            return Ok(None);
        };
        let session = totals_in(&tx, owner, id)?;
        let last_turn = tx
            .query_row(
                "SELECT turn_id,COALESCE(outcome,'interrupted'),COALESCE(usage_at,created_at),usage FROM turns
             WHERE owner_id = ?1 AND session_id=?2 AND state='ended' ORDER BY created_at DESC,rowid DESC LIMIT 1",
                params![owner, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .map(|(turn_id, outcome, observed_at, raw)| -> Result<_> {
                Ok(UsageTurn {
                    turn_id,
                    outcome,
                    observed_at,
                    usage: raw.map(|s| serde_json::from_str(&s)).transpose()?,
                })
            })
            .transpose()?;
        // Match the existing family reader's owner, hat and deletion boundaries.
        let root: Option<String> = tx.query_row(
            "SELECT f.root_id FROM session_families f JOIN sessions r ON r.owner_id = ?1 AND r.id=f.root_id AND r.hat_id=?3 AND r.lifecycle<>'deleted'
             WHERE f.owner_id = ?1 AND f.child_id=?2 AND f.hat_id=?3",params![owner,id,hat],|r|r.get(0)).optional()?;
        let root = root.as_deref().unwrap_or(id);
        let mut stmt = tx.prepare(
            "SELECT s.id FROM (SELECT ?2 AS id UNION ALL SELECT f.child_id FROM session_families f
                 WHERE f.owner_id = ?1 AND f.root_id=?2 AND f.hat_id=?3) members
             JOIN sessions s ON s.id=members.id AND s.owner_id = ?1 AND s.hat_id=?3 AND s.lifecycle<>'deleted'
             ORDER BY s.id LIMIT 257",
        )?;
        let ids = stmt
            .query_map(params![owner, root, hat], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let truncated = ids.len() > 256;
        let mut family = FamilyUsage {
            totals: empty_totals(),
            sessions: ids.len(),
            reporting_sessions: 0,
            cost_reporting_sessions: 0,
            truncated,
        };
        for member in ids.iter().take(256) {
            let t = if member == id {
                session.clone()
            } else {
                totals_in(&tx, owner, member)?
            };
            family.totals.turns += t.turns;
            family.totals.reported_turns += t.reported_turns;
            if let Some(tokens) = t.tokens {
                family.totals.partial |= !add_tokens(&mut family.totals.tokens, &tokens);
                family.reporting_sessions += 1;
            }
            family.totals.partial |= t.partial;
            if let Some(cost) = t.cost
                && family.totals.cost.as_ref().is_none_or(|c| c.currency == cost.currency)
            {
                if let Some(sum) = &mut family.totals.cost {
                    sum.amount += cost.amount;
                } else {
                    family.totals.cost = Some(cost);
                }
                family.cost_reporting_sessions += 1;
            }
            if let Some(since) = t.since
                && family.totals.since.as_ref().is_none_or(|s| since < *s)
            {
                family.totals.since = Some(since);
            }
        }
        family.totals.partial |= truncated
            || family.reporting_sessions < family.sessions
            || family.totals.reported_turns < family.totals.turns;
        if let Some(cost) = &mut family.totals.cost {
            cost.partial = true;
        }
        Ok(Some(SessionUsage {
            session_id: id.into(),
            source,
            context: context.map(|s| serde_json::from_str(&s)).transpose()?,
            last_turn,
            session,
            family,
            retention: RetentionState {
                supported: attached.is_some(),
                requested: retention_in(&tx, owner, id)?,
                attached_request: attached.map(|s| serde_json::from_str(&s)).transpose()?,
            },
        }))
    }

    /// Returns false for unknown/deleted/unsupported sessions.
    pub fn set_cache_retention(&self, id: &str, requested: CacheRetention) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let value = serde_json::to_value(requested)?;
        let changed = tx.execute(
            "UPDATE sessions SET cache_retention=?3 WHERE owner_id = ?1 AND id=?2 AND lifecycle<>'deleted'
             AND (cache_retention_attached IS NOT NULL OR ?3='inherit')
             AND NOT EXISTS(SELECT 1 FROM purged_hats h WHERE h.owner_id = ?1 AND h.hat_id=sessions.hat_id)",
            params![self.owner_id(), id, value.as_str()],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }
}
