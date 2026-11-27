//! Shared session identity and delivered delegation grants. This seam never
//! imports sessions, exports a bearer upstream, or trusts caller arguments.
use crate::revocation::{Cut, Revocations, Watch};
use crate::scope::{Principal, PrincipalKind};
use crate::store::GatewayStore;
use crate::tokens::{is_session_token, token_hash};
use anyhow::Result;
use hennery_proto::delegation::HatSessionTools;
use hennery_proto::frames::{McpServer, NameValue};
use rusqlite::{OptionalExtension, Transaction, params};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// Opaque generation fence. Debug intentionally reveals no hash.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenGeneration(String);
impl TokenGeneration {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for TokenGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenGeneration(<redacted>)")
    }
}
#[derive(Debug, Clone)]
pub struct SessionIdentity {
    pub owner_id: String,
    pub principal: Principal,
    generation: TokenGeneration,
    delivered_revision: Option<i64>,
    delivered_peer_revision: Option<i64>,
}
impl SessionIdentity {
    pub fn generation(&self) -> &TokenGeneration {
        &self.generation
    }
    pub fn delivered_delegation_revision(&self) -> Option<i64> {
        self.delivered_revision
    }
    pub fn delivered_peer_revision(&self) -> Option<i64> {
        self.delivered_peer_revision
    }
}

/// Cannot be constructed from connection URLs, tool JSON or arbitrary names.
pub(crate) struct BuiltinDescriptor {
    pub revision: i64,
}
impl BuiltinDescriptor {
    pub fn server(self, origin: &str, token: &str) -> McpServer {
        McpServer::Http {
            name: "hennery".into(),
            url: format!("{origin}/mcp/builtin/hennery"),
            headers: vec![NameValue::new("Authorization", format!("Bearer {token}"))],
        }
    }
}

pub(crate) fn setting_in(tx: &Transaction<'_>, owner: &str, hat: &str) -> Result<Option<HatSessionTools>> {
    Ok(tx
        .query_row(
            "SELECT COALESCE(t.delegation_enabled, 0), COALESCE(t.revision, 0)
        FROM hats h LEFT JOIN hat_session_tools t ON t.owner_id = ?1 AND t.hat_id = h.id
        WHERE h.owner_id = ?1 AND h.id = ?2
        AND NOT EXISTS (SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id = h.id)",
            params![owner, hat],
            |r| {
                Ok(HatSessionTools {
                    delegation_enabled: r.get(0)?,
                    revision: r.get(1)?,
                })
            },
        )
        .optional()?)
}
pub(crate) fn builtin_in(tx: &Transaction<'_>, owner: &str, hat: &str) -> Result<Option<BuiltinDescriptor>> {
    Ok(setting_in(tx, owner, hat)?
        .filter(|s| s.delegation_enabled)
        .map(|s| BuiltinDescriptor { revision: s.revision }))
}

type WatchRegistry = Vec<(Weak<hennery_kernel::coordination::Coordination>, Arc<Revocations>)>;
pub(crate) fn grant_watches(coordination: Arc<hennery_kernel::coordination::Coordination>) -> Arc<Revocations> {
    static REGISTRY: OnceLock<Mutex<WatchRegistry>> = OnceLock::new();
    let mut registry = REGISTRY.get_or_init(Mutex::default).lock().expect("grant registry");
    registry.retain(|(key, _)| key.strong_count() > 0);
    if let Some(watches) = registry.iter().find_map(|(key, watches)| {
        key.upgrade()
            .filter(|key| Arc::ptr_eq(key, &coordination))
            .map(|_| watches.clone())
    }) {
        return watches;
    }
    let watches = Arc::new(Revocations::new());
    registry.push((Arc::downgrade(&coordination), watches.clone()));
    watches
}
fn grant_key(owner: &str, hat: &str, revision: i64) -> String {
    format!("{owner}:{hat}:{revision}")
}

/// Resolve generation in the caller's transaction: starting sessions have the
/// same committed identity as active ones. Lifecycle is the sessions module's.
pub fn resolve_in(tx: &Transaction<'_>, owner: &str, generation: &TokenGeneration) -> Result<Option<SessionIdentity>> {
    Ok(tx
        .query_row(
            "SELECT t.session_id, t.host_id, t.hat_id, t.delivered_delegation_revision, t.delivered_peer_revision
        FROM gw_session_tokens t JOIN hosts h ON h.id = t.host_id AND h.owner_id = ?1
        WHERE t.owner_id = ?1 AND t.token_hash = ?2 AND t.revoked_at IS NULL AND h.revoked_at IS NULL
        AND NOT EXISTS (SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id = t.hat_id)",
            params![owner, generation.as_str()],
            |r| {
                Ok(SessionIdentity {
                    owner_id: owner.into(),
                    principal: Principal {
                        hat_id: r.get(2)?,
                        kind: PrincipalKind::Session {
                            session_id: r.get(0)?,
                            host_id: r.get(1)?,
                        },
                    },
                    generation: generation.clone(),
                    delivered_revision: r.get(3)?,
                    delivered_peer_revision: r.get(4)?,
                })
            },
        )
        .optional()?)
}
/// Recheck the exact accepted generation and policy revision in a mutation's
/// own transaction. A fresh token cannot authorize an old queued revision.
pub fn granted_in(
    tx: &Transaction<'_>,
    owner: &str,
    identity: &SessionIdentity,
    accepted_revision: i64,
) -> Result<bool> {
    if identity.owner_id != owner {
        return Ok(false);
    }
    let Some(current) = resolve_in(tx, owner, identity.generation())? else {
        return Ok(false);
    };
    if current.principal != identity.principal || current.delivered_revision != Some(accepted_revision) {
        return Ok(false);
    }
    Ok(setting_in(tx, owner, &current.principal.hat_id)?
        .is_some_and(|s| s.delegation_enabled && s.revision == accepted_revision))
}
impl GatewayStore {
    pub fn session_tools(&self, hat: &str) -> Result<Option<HatSessionTools>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        setting_in(&tx, self.owner_id(), hat)
    }
    pub fn set_session_tools(&self, hat: &str, enabled: bool) -> Result<Option<HatSessionTools>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some(before) = setting_in(&tx, self.owner_id(), hat)? else {
            return Ok(None);
        };
        if before.delegation_enabled == enabled {
            return Ok(Some(before));
        }
        let revision = before
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("grant revision exhausted"))?;
        tx.execute(
            "INSERT INTO hat_session_tools(owner_id, hat_id, delegation_enabled, revision)
            VALUES (?1, ?2, ?3, ?4) ON CONFLICT(owner_id, hat_id) DO UPDATE SET
            delegation_enabled = excluded.delegation_enabled, revision = excluded.revision
            WHERE hat_session_tools.owner_id = ?1",
            params![self.owner_id(), hat, enabled, revision],
        )?;
        tx.commit()?;
        self.delegation_watches
            .cut(Cut(vec![grant_key(self.owner_id(), hat, before.revision)]));
        Ok(Some(HatSessionTools {
            delegation_enabled: enabled,
            revision,
        }))
    }
    pub fn authenticate_session(&self, token: &str) -> Result<Option<SessionIdentity>> {
        if !is_session_token(token) {
            return Ok(None);
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        resolve_in(&tx, self.owner_id(), &TokenGeneration(token_hash(token)))
    }
    pub fn delegation_granted(&self, identity: &SessionIdentity) -> Result<Option<i64>> {
        let Some(revision) = identity.delivered_revision else {
            return Ok(None);
        };
        Ok(self.recheck_delegation(identity, revision)?.then_some(revision))
    }
    pub fn recheck_delegation(&self, identity: &SessionIdentity, revision: i64) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        granted_in(&tx, self.owner_id(), identity, revision)
    }
    /// Install under the same owner gate as policy mutation, before resolving
    /// its current grant. The token watch must already exist before auth.
    pub fn watch_delegation(&self, identity: &SessionIdentity) -> Result<Option<Watch>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(revision) = identity.delivered_revision else {
            return Ok(None);
        };
        let watch = self
            .delegation_watches
            .watch(&grant_key(self.owner_id(), &identity.principal.hat_id, revision));
        Ok(granted_in(&tx, self.owner_id(), identity, revision)?.then_some(watch))
    }
    pub fn watch_session(&self, token: &str, revocations: &Revocations) -> Option<Watch> {
        is_session_token(token).then(|| revocations.watch(&token_hash(token)))
    }
}
