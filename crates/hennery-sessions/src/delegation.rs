//! D1's internal verified caller and family storage. No handler receives the
//! bearer; upstream plug-ins continue using C1 namespace authentication.
use crate::store::Store;
use anyhow::Result;
use hennery_gateway::identity::{SessionIdentity, TokenGeneration, granted_in};
use hennery_gateway::scope::PrincipalKind;
use hennery_proto::delegation::SessionFamily;
use rusqlite::{OptionalExtension, Transaction, params};

struct CallerRow {
    cwd: String,
    agent: String,
    capabilities: String,
    isolation: Option<String>,
    mode: String,
}

#[derive(Debug, Clone)]
pub struct SessionToolContext {
    owner_id: String,
    session_id: String,
    host_id: String,
    hat_id: String,
    canonical_cwd: String,
    agent: String,
    token_generation: TokenGeneration,
    delivered_revision: i64,
}
impl SessionToolContext {
    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn host_id(&self) -> &str {
        &self.host_id
    }
    pub fn hat_id(&self) -> &str {
        &self.hat_id
    }
    pub fn canonical_cwd(&self) -> &str {
        &self.canonical_cwd
    }
    pub fn agent(&self) -> &str {
        &self.agent
    }
    pub fn generation(&self) -> &TokenGeneration {
        &self.token_generation
    }
    pub fn grant_revision(&self) -> i64 {
        self.delivered_revision
    }
}
impl Store {
    pub fn session_tool_context(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        mutation: bool,
    ) -> Result<Option<SessionToolContext>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        self.session_tool_context_in(&tx, identity, revision, mutation)
    }
    /// Reads and future mutations use this exact transaction-bound predicate.
    pub fn session_tool_context_in(
        &self,
        tx: &Transaction<'_>,
        identity: &SessionIdentity,
        revision: i64,
        mutation: bool,
    ) -> Result<Option<SessionToolContext>> {
        if !granted_in(tx, self.owner_id(), identity, revision)? {
            return Ok(None);
        }
        let PrincipalKind::Session { session_id, host_id } = &identity.principal.kind;
        let row: Option<CallerRow> = tx.query_row(
            "SELECT s.cwd, s.agent, h.capabilities, h.mcp_isolation, s.mcp_delivery_mode
             FROM sessions s JOIN hosts h ON h.id = s.host_id AND h.owner_id = ?1
             WHERE s.owner_id = ?1 AND s.id = ?2 AND s.host_id = ?3 AND s.hat_id = ?4
               AND s.close_requested = 0 AND s.lifecycle IN ('starting', 'active') AND s.presumed_parked = 0
               AND (?5 = 0 OR s.lifecycle = 'active') AND h.revoked_at IS NULL
               AND (s.mcp_delivery_mode IN ('isolated','managed_plus_native') OR (s.mcp_delivery_mode = 'unisolated' AND s.hat_id = h.default_hat_id))
               AND NOT EXISTS (SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id = s.hat_id)",
             params![self.owner_id(), session_id, host_id, identity.principal.hat_id, mutation],
             |r| Ok(CallerRow { cwd: r.get(0)?, agent: r.get(1)?, capabilities: r.get(2)?, isolation: r.get(3)?, mode: r.get(4)? })).optional()?;
        let Some(row) = row else { return Ok(None) };
        let capabilities =
            serde_json::from_str::<hennery_proto::frames::Capabilities>(&row.capabilities).unwrap_or_default();
        let isolation = row
            .isolation
            .as_deref()
            .and_then(|value| serde_json::from_str::<hennery_proto::frames::AgentIsolation>(value).ok())
            .unwrap_or_default();
        if !crate::mcp_source::delivery_supported(&row.mode, &capabilities, isolation.get(&row.agent)) {
            return Ok(None);
        }
        let (cwd, agent) = (row.cwd, row.agent);
        if !hennery_kernel::hats::is_canonical(&cwd) {
            return Ok(None);
        }
        Ok(Some(SessionToolContext {
            owner_id: self.owner_id().into(),
            session_id: session_id.clone(),
            host_id: host_id.clone(),
            hat_id: identity.principal.hat_id.clone(),
            canonical_cwd: cwd,
            agent,
            token_generation: identity.generation().clone(),
            delivered_revision: revision,
        }))
    }
    /// D2 calls this only after allocating a new starting child in this same
    /// transaction. No public endpoint can adopt an unrelated session.
    pub fn bind_family_in(
        &self,
        tx: &Transaction<'_>,
        identity: &SessionIdentity,
        revision: i64,
        child: &str,
        created_event_id: i64,
    ) -> Result<bool> {
        let Some(caller) = self.session_tool_context_in(tx, identity, revision, true)? else {
            return Ok(false);
        };
        let root: String = tx.query_row("SELECT COALESCE((SELECT root_id FROM session_families WHERE owner_id = ?1 AND child_id = ?2 AND detached = 0), ?2)",
            params![self.owner_id(), caller.session_id()], |r| r.get(0))?;
        // The trigger independently enforces immutable scope and no cycles.
        let valid: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions c
            WHERE c.owner_id = ?1 AND c.id = ?2 AND c.hat_id = ?3 AND c.host_id = ?4
              AND c.lifecycle = 'starting' AND c.close_requested = 0 AND c.id <> ?5 AND c.id <> ?6
              AND NOT EXISTS (SELECT 1 FROM session_families f WHERE f.owner_id = ?1 AND f.child_id = c.id))",
            params![
                self.owner_id(),
                child,
                caller.hat_id(),
                caller.host_id(),
                caller.session_id(),
                root
            ],
            |r| r.get(0),
        )?;
        if !valid {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO session_families(owner_id, child_id, parent_id, root_id, hat_id, created_event_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                self.owner_id(),
                child,
                caller.session_id(),
                root,
                caller.hat_id(),
                created_event_id
            ],
        )?;
        Ok(true)
    }
    pub fn direct_child(
        &self,
        identity: &SessionIdentity,
        revision: i64,
        child: &str,
    ) -> Result<Option<SessionFamily>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(caller) = self.session_tool_context_in(&tx, identity, revision, true)? else {
            return Ok(None);
        };
        Ok(tx
            .query_row(
                "SELECT f.child_id, f.parent_id, f.root_id, f.hat_id, f.created_event_id, f.detached, f.reason
            FROM session_families f JOIN sessions c ON c.id = f.child_id AND c.owner_id = ?1
            JOIN sessions r ON r.id = f.root_id AND r.owner_id = ?1
            WHERE f.owner_id = ?1 AND f.parent_id = ?2 AND f.child_id = ?3 AND f.hat_id = ?4 AND f.detached = 0
              AND c.hat_id = ?4 AND r.hat_id = ?4 AND c.close_requested = 0 AND r.close_requested = 0
              AND c.lifecycle NOT IN ('closed', 'deleted') AND r.lifecycle NOT IN ('closed', 'deleted')",
                params![self.owner_id(), caller.session_id(), child, caller.hat_id()],
                |r| {
                    Ok(SessionFamily {
                        child_id: r.get(0)?,
                        parent_id: r.get(1)?,
                        root_id: r.get(2)?,
                        hat_id: r.get(3)?,
                        created_event_id: r.get(4)?,
                        detached: r.get(5)?,
                        reason: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }
}

impl Store {
    /// D1 allocation primitive for D2's validated launch path. The session,
    /// family assignment, delivered revision and token share one commit.
    #[allow(clippy::too_many_arguments)]
    pub fn create_delegated_session(
        &self,
        parent: &SessionIdentity,
        revision: i64,
        child_id: &str,
        agent: &str,
        canonical_cwd: &str,
        mcp: crate::store::McpContext,
        created_event_id: i64,
    ) -> Result<Option<crate::store::McpGiven>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some(caller) = self.session_tool_context_in(&tx, parent, revision, true)? else {
            return Ok(None);
        };
        if !hennery_kernel::hats::is_canonical(canonical_cwd) {
            return Ok(None);
        }
        let ts = crate::store::now();
        tx.execute(
            "INSERT INTO sessions(id, owner_id, host_id, hat_id, agent, cwd, lifecycle, created_at, last_event_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'starting', ?7, ?7)",
            params![
                child_id,
                self.owner_id(),
                caller.host_id(),
                caller.hat_id(),
                agent,
                canonical_cwd,
                ts
            ],
        )?;
        if !self.bind_family_in(&tx, parent, revision, child_id, created_event_id)? {
            return Ok(None);
        }
        let (given, _new_child_cut) = self.deliver_in(&tx, child_id, caller.host_id(), caller.hat_id(), mcp, &ts)?;
        tx.commit()?;
        Ok(Some(given))
    }
}
