//! Independent, generation-bound access to same-hat peer sessions.
use crate::{
    identity::{SessionIdentity, resolve_in},
    revocation::{Cut, Watch},
    store::GatewayStore,
};
use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, params};

pub use hennery_proto::peers::{PeerAccess, PeerPolicy};

pub(crate) fn setting_in(tx: &Transaction<'_>, owner: &str, hat: &str) -> Result<Option<PeerPolicy>> {
    let row: Option<(String, i64)> = tx
        .query_row(
            "SELECT COALESCE(p.access,'off'),COALESCE(p.revision,0) FROM hats h
         LEFT JOIN hat_peer_access p ON p.owner_id = ?1 AND p.hat_id=h.id
         WHERE h.owner_id = ?1 AND h.id=?2
         AND NOT EXISTS(SELECT 1 FROM purged_hats p WHERE p.owner_id = ?1 AND p.hat_id=h.id)",
            params![owner, hat],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(access, revision)| {
        Ok(PeerPolicy {
            access: serde_json::from_value(serde_json::Value::String(access))?,
            revision,
        })
    })
    .transpose()
}
fn watch_key(owner: &str, hat: &str, revision: i64) -> String {
    format!("peer:{owner}:{hat}:{revision}")
}

pub fn granted_in(
    tx: &Transaction<'_>,
    owner: &str,
    identity: &SessionIdentity,
    revision: i64,
    write: bool,
) -> Result<bool> {
    if identity.owner_id != owner {
        return Ok(false);
    }
    let Some(current) = resolve_in(tx, owner, identity.generation())? else {
        return Ok(false);
    };
    if current.principal != identity.principal {
        return Ok(false);
    }
    let delivered: Option<i64> = tx.query_row(
        "SELECT delivered_peer_revision FROM gw_session_tokens WHERE owner_id = ?1 AND token_hash=?2 AND revoked_at IS NULL",
        params![owner,identity.generation().as_str()], |r| r.get(0)).optional()?.flatten();
    Ok(delivered == Some(revision)
        && setting_in(tx, owner, &current.principal.hat_id)?
            .is_some_and(|p| p.revision == revision && p.access.enabled() && (!write || p.access.writable())))
}
impl GatewayStore {
    pub fn peer_policy(&self, hat: &str) -> Result<Option<PeerPolicy>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        setting_in(&tx, self.owner_id(), hat)
    }
    pub fn set_peer_policy(&self, hat: &str, access: PeerAccess) -> Result<Option<PeerPolicy>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some(before) = setting_in(&tx, self.owner_id(), hat)? else {
            return Ok(None);
        };
        if before.access == access {
            return Ok(Some(before));
        }
        let revision = before
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("peer revision exhausted"))?;
        tx.execute(
            "INSERT INTO hat_peer_access(owner_id,hat_id,access,revision) VALUES(?1,?2,?3,?4)
            ON CONFLICT(owner_id,hat_id) DO UPDATE SET access=excluded.access,revision=excluded.revision
            WHERE hat_peer_access.owner_id = ?1",
            params![self.owner_id(), hat, access.as_str(), revision],
        )?;
        tx.commit()?;
        self.delegation_watches
            .cut(Cut(vec![watch_key(self.owner_id(), hat, before.revision)]));
        Ok(Some(PeerPolicy { access, revision }))
    }
    pub fn watch_peers(&self, identity: &SessionIdentity) -> Result<Option<(Watch, i64)>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(policy) = setting_in(&tx, self.owner_id(), &identity.principal.hat_id)? else {
            return Ok(None);
        };
        let watch =
            self.delegation_watches
                .watch(&watch_key(self.owner_id(), &identity.principal.hat_id, policy.revision));
        Ok(granted_in(&tx, self.owner_id(), identity, policy.revision, false)?.then_some((watch, policy.revision)))
    }
}
