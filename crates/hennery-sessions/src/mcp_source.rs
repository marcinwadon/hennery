//! Attach-bound source-policy admission and acknowledgement. Managed mount
//! selection and token lifetime remain in the existing delivery transaction.
use crate::store::{McpContext, Store};
use anyhow::Result;
use hennery_proto::frames::{Capabilities, Capability, McpIsolation, SessionBody};
use hennery_proto::mcp_source::{
    McpSourceOverride, McpSourcePolicy, McpSourceSnapshot, McpSourceState, source_policy_for,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::BTreeMap;

pub const UNSUPPORTED: &str = "mcp_source_policy_unsupported";

#[derive(Debug)]
pub struct Unsupported;
impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(UNSUPPORTED)
    }
}
impl std::error::Error for Unsupported {}

// Collector-only eligibility metadata. Old stored snapshots deserialize with
// their original eligibility; public snapshots retain their historical fields.
#[derive(serde::Deserialize, serde::Serialize)]
struct SourceRequest {
    #[serde(flatten)]
    snapshot: McpSourceSnapshot,
    #[serde(default)]
    invalidated: bool,
}

pub(crate) fn invalidate_in(tx: &Transaction<'_>, owner: &str, session: &str) -> Result<()> {
    let requested: Option<String> = tx.query_row(
        "SELECT mcp_source_requested FROM sessions WHERE owner_id = ?1 AND id = ?2",
        params![owner, session],
        |r| r.get(0),
    )?;
    if let Some(requested) = requested {
        let mut requested: SourceRequest = serde_json::from_str(&requested)?;
        requested.invalidated = true;
        tx.execute(
            "UPDATE sessions SET mcp_source_requested = ?3 WHERE owner_id = ?1 AND id = ?2",
            params![owner, session, serde_json::to_string(&requested)?],
        )?;
    }
    Ok(())
}

pub(crate) fn requested_in(
    tx: &Transaction<'_>,
    owner: &str,
    session: &str,
    hat: &str,
    context: McpContext,
) -> Result<Option<McpSourceSnapshot>> {
    // Preserve correlation if an API client returns an explicit session to
    // compatibility mode. A late failure from its previous attach must not
    // fail this one. Never require an explicit policy echo from an old host.
    let (tracked, agent): (bool, String) = tx.query_row(
        "SELECT mcp_source_requested IS NOT NULL OR mcp_source_applied IS NOT NULL, agent
         FROM sessions WHERE owner_id = ?1 AND id = ?2",
        params![owner, session],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let (policy, revision) = policy_in(tx, owner, hat, &agent, context)?;
    let snapshot = (policy != McpSourcePolicy::Legacy || tracked).then(|| McpSourceSnapshot {
        policy,
        revision,
        request_id: uuid::Uuid::now_v7().to_string(),
    });
    tx.execute(
        "UPDATE sessions SET mcp_source_requested = ?3 WHERE owner_id = ?1 AND id = ?2",
        params![
            owner,
            session,
            snapshot.as_ref().map(serde_json::to_string).transpose()?
        ],
    )?;
    Ok(snapshot)
}

pub(crate) fn selected_in(
    conn: &rusqlite::Connection,
    owner: &str,
    hat: &str,
    agent: &str,
) -> Result<(McpSourcePolicy, u64)> {
    let row: Option<(String, u64, String)> = conn
        .query_row(
            "SELECT mcp_source_policy, mcp_policy_revision, mcp_source_overrides FROM hats WHERE owner_id = ?1 AND id = ?2",
            params![owner, hat],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)),
        )
        .optional()?;
    let (text, revision, overrides) = row.unwrap_or(("legacy".into(), 0, "{}".into()));
    let fallback = McpSourcePolicy::parse(&text).ok_or_else(|| anyhow::anyhow!("invalid stored MCP source policy"))?;
    let overrides: BTreeMap<String, McpSourceOverride> = serde_json::from_str(&overrides)?;
    Ok(source_policy_for(fallback, revision, &overrides, agent))
}

fn policy_in(
    conn: &rusqlite::Connection,
    owner: &str,
    hat: &str,
    agent: &str,
    context: McpContext,
) -> Result<(McpSourcePolicy, u64)> {
    let (policy, revision) = selected_in(conn, owner, hat, agent)?;
    let mut caps = Vec::new();
    if context.capable {
        caps.push(Capability::McpServers);
    }
    if context.source_policy {
        caps.push(Capability::McpSourcePolicy);
    }
    if context.native_sources {
        caps.push(Capability::McpNativeSources);
    }
    if !policy.supported(&Capabilities(caps), context.isolation) {
        return Err(Unsupported.into());
    }
    Ok((policy, revision))
}

/// Validate before lifecycle/configuration can be changed by a late fact.
pub(crate) fn fact_matches(tx: &Transaction<'_>, owner: &str, session: &str, body: &SessionBody) -> Result<bool> {
    let (request, report, indexed) = match body {
        SessionBody::SessionStarted {
            request_id, indexed, ..
        } => (request_id, indexed.mcp_source_policy.as_ref(), Some(indexed)),
        SessionBody::StartFailed { request_id, .. } => (request_id, None, None),
        _ => return Ok(true),
    };
    let expected: Option<String> = tx.query_row(
        "SELECT mcp_source_requested FROM sessions WHERE owner_id = ?1 AND id = ?2",
        params![owner, session],
        |r| r.get(0),
    )?;
    let Some(expected) = expected else {
        return Ok(report.is_none());
    };
    let stored: SourceRequest = serde_json::from_str(&expected)?;
    if stored.invalidated {
        return Ok(false);
    }
    let expected = stored.snapshot;
    if request != &expected.request_id {
        return Ok(false);
    }
    if indexed.is_none() {
        return Ok(true);
    }
    if !(report == Some(&expected) || (expected.policy == McpSourcePolicy::Legacy && report.is_none())) {
        return Ok(false);
    }
    let attach: Option<(u64, Option<String>)> = tx
        .query_row(
            "SELECT attach_generation, request_id FROM session_attach_evidence WHERE owner_id = ?1 AND session_id = ?2",
            params![owner, session],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)),
        )
        .optional()?;
    // Only a delegated request owns its stored generation. A later ordinary
    // resume uses generation zero; ingest_delegation_in resets the old evidence
    // after this admission check. The source request above still fences lateness.
    let generation = attach
        .filter(|(_, attach_request)| attach_request.as_deref() == Some(request))
        .map_or(0, |(generation, _)| generation);
    let evidence = indexed.and_then(|i| i.config_evidence.as_deref());
    Ok(match evidence {
        Some(evidence) => evidence.attach_generation == generation,
        None => generation == 0,
    })
}

impl Store {
    /// Preflight before a disposable runtime probe. Actual attach repeats this
    /// under its transaction, so a concurrent hat edit cannot bypass admission.
    pub fn check_mcp_source_policy(&self, hat: &str, agent: &str, context: McpContext) -> Result<()> {
        policy_in(&self.conn(), self.owner_id(), hat, agent, context).map(|_| ())
    }

    pub fn mcp_source_state(&self, session: &str) -> Result<Option<McpSourceState>> {
        let row: Option<(String, u64, Option<String>, Option<String>, String, String)> = self
            .conn()
            .query_row(
                "SELECT h.mcp_source_policy, h.mcp_policy_revision, s.mcp_source_requested, s.mcp_source_applied, h.mcp_source_overrides, s.agent
                 FROM sessions s JOIN hats h ON h.owner_id = s.owner_id AND h.id = s.hat_id
                 WHERE s.owner_id = ?1 AND s.id = ?2 AND s.lifecycle <> 'deleted'",
                params![self.owner_id(), session],
                |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        row.map(|(policy, revision, requested, applied, overrides, agent)| {
            let fallback =
                McpSourcePolicy::parse(&policy).ok_or_else(|| anyhow::anyhow!("invalid stored MCP source policy"))?;
            let (policy, revision) = source_policy_for(fallback, revision, &serde_json::from_str(&overrides)?, &agent);
            Ok(McpSourceState {
                policy,
                revision,
                requested: requested.map(|v| serde_json::from_str(&v)).transpose()?,
                applied: applied.map(|v| serde_json::from_str(&v)).transpose()?,
            })
        })
        .transpose()
    }
}

/// Current-profile eligibility for explicit combined sessions. This does not
/// grant mounts or replace any owner/hat/host/family check.
pub fn combined_supported(caps: &Capabilities, isolation: McpIsolation) -> bool {
    McpSourcePolicy::ManagedPlusNative.supported(caps, isolation)
}

pub fn delivery_supported(mode: &str, caps: &Capabilities, isolation: McpIsolation) -> bool {
    if !caps.has(Capability::McpServers) {
        return false;
    }
    match mode {
        "isolated" => isolation != McpIsolation::None,
        "managed_plus_native" => combined_supported(caps, isolation),
        // SQL predicates separately require the current default hat.
        "unisolated" => true,
        _ => false,
    }
}
