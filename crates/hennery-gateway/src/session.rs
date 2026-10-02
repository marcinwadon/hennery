//! What a session gets of the gateway (ACP core §1, gateway spec §3.2),
//! through the one seam the sessions module reaches it by (umbrella §9):
//! `SessionMcp`. The sessions store calls it inside the transactions of its
//! own transitions (lane L1): a start's or resume's servers are read and
//! its token minted before the transition commits, so a failed mint rolls
//! it back; a park's, close's or revoke's token is revoked in that
//! transition. What a transaction invalidated is cut (`Revocations`) only
//! once it has committed (plan 8e decision 12).
//!
//! The delivery decision (umbrella §8.5) is the sessions module's (lane
//! L2): it owns `sessions.hat_id`, and passes the decision in. This amends
//! ACP core §1's `servers_for(host, hat, session)`.
//!
//! Every SQL statement here names the owner; the owner audit reads this
//! file (`hennery-testkit/tests/owner_filter.rs`).

use crate::api::GatewayState;
use crate::key::MasterKey;
use crate::revocation::{Cut, Revocations};
use crate::store::GatewayStore;
use crate::{stdio, tokens};
use anyhow::{Result, anyhow};
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::frames::{McpServer, NameValue};
use hennery_proto::rest::McpSessionDeliveryMode;
use rusqlite::{Transaction, params};
use std::sync::Arc;

/// The session a start or resume is for, as its row says.
#[derive(Debug, Clone, Copy)]
pub struct SessionRef<'a> {
    pub session_id: &'a str,
    pub host_id: &'a str,
    pub hat_id: &'a str,
}

/// What a start or resume gets: its servers, and the token it superseded
/// or revoked, to cut once the transaction commits.
#[derive(Debug)]
pub struct Delivered {
    /// `hennery-<slug>` HTTP entries first (the hat's connections mounted
    /// on the host, each with the session's token), then the stdio
    /// servers. Their values are secrets: they go into the session's frame
    /// and nowhere else.
    pub servers: Vec<McpServer>,
    pub cut: Cut,
}

/// The gateway, as the sessions module sees it (umbrella §9, ACP core §1).
pub trait SessionMcp: Send + Sync {
    /// The servers of `session`'s start or resume, inside its transaction:
    /// with a delivering `mode`, the hat's connections mounted on the host,
    /// with a fresh token minted (superseding the session's previous one),
    /// then the hat's stdio servers on that host. With any other mode none,
    /// and the previous token revoked. An error should roll the transition
    /// back.
    fn servers_in(
        &self,
        tx: &Transaction<'_>,
        session: SessionRef<'_>,
        mode: McpSessionDeliveryMode,
    ) -> Result<Delivered>;

    /// Revoke the session's token inside the caller's transaction (lane
    /// L4's sites). Revoking twice, or a session that never had one, is
    /// not an error.
    fn revoke_in(&self, tx: &Transaction<'_>, session_id: &str) -> Result<Cut>;

    /// Revoke every live token of a host (a host revoke, ACP core §4.8).
    fn revoke_host_in(&self, tx: &Transaction<'_>, host_id: &str) -> Result<Cut>;

    /// End what is open on the tokens `cut` names. Only after the
    /// transaction that produced it has committed.
    fn cut(&self, cut: Cut);

    /// The gateway's part of a hat's purge (kernel spec §5.5, lane L6), in
    /// its own transaction, cut once it commits. Idempotent.
    fn purge_hat(&self, hat_id: &str) -> Result<()>;
}

/// No gateway: no servers, nothing to revoke. A sessions store opened on
/// its own (its tests) has this until the collector gives it the gateway.
pub struct NoSessionMcp;

impl SessionMcp for NoSessionMcp {
    fn servers_in(&self, _: &Transaction<'_>, _: SessionRef<'_>, _: McpSessionDeliveryMode) -> Result<Delivered> {
        Ok(Delivered {
            servers: Vec::new(),
            cut: Cut::default(),
        })
    }

    fn revoke_in(&self, _: &Transaction<'_>, _: &str) -> Result<Cut> {
        Ok(Cut::default())
    }

    fn revoke_host_in(&self, _: &Transaction<'_>, _: &str) -> Result<Cut> {
        Ok(Cut::default())
    }

    fn cut(&self, _: Cut) {}

    fn purge_hat(&self, _: &str) -> Result<()> {
        Ok(())
    }
}

/// The collector's gateway, on the same `hennery.db` as the sessions
/// store, whose transactions it is handed.
pub struct GatewayMcp {
    store: Arc<GatewayStore>,
    key: Arc<MasterKey>,
    operator: Arc<Operator>,
    revocations: Revocations,
}

impl GatewayMcp {
    /// On `gateway`'s store, key and owner, cutting through the same
    /// `Revocations` its proxy watches.
    pub fn new(gateway: &GatewayState) -> Self {
        Self {
            store: gateway.store.clone(),
            key: gateway.key.clone(),
            operator: gateway.operator.clone(),
            revocations: gateway.revocations.clone(),
        }
    }

    fn owner(&self) -> &str {
        self.store.owner_id()
    }
}

/// The slugs of `hat_id`'s connections mounted on `host_id`, a host that is
/// not revoked: what `MountPolicy::connection` would let the session's
/// token reach, with the same joins. Oldest first.
fn mounted_slugs_in(tx: &Transaction<'_>, owner: &str, host_id: &str, hat_id: &str) -> Result<Vec<String>> {
    let mut stmt = tx.prepare(
        "SELECT c.slug FROM gw_connections c
         WHERE c.owner_id = ?1 AND c.hat_id = ?2
             AND EXISTS (SELECT 1 FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
                         WHERE m.connection_id = c.id AND m.owner_id = ?1 AND m.host_id = ?3
                             AND h.revoked_at IS NULL)
         ORDER BY c.slug",
    )?;
    let rows = stmt.query_map(params![owner, hat_id, host_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

impl SessionMcp for GatewayMcp {
    fn servers_in(
        &self,
        tx: &Transaction<'_>,
        session: SessionRef<'_>,
        mode: McpSessionDeliveryMode,
    ) -> Result<Delivered> {
        let owner = self.owner();
        let now = unix_now();
        // Whatever this start or resume gets, the token before it is done.
        let cut = Cut(tokens::session_hash_in(tx, owner, session.session_id)?
            .into_iter()
            .collect());
        // Nothing to deliver; the mode decides. (A session from before hats,
        // hat "", is never given a server: no connection or stdio server
        // can name that hat, their foreign keys refuse it.)
        if !mode.delivers() {
            tokens::revoke_in(tx, owner, session.session_id, now)?;
            return Ok(Delivered {
                servers: Vec::new(),
                cut,
            });
        }
        let slugs = mounted_slugs_in(tx, owner, session.host_id, session.hat_id)?;
        let mut servers = Vec::with_capacity(slugs.len());
        if slugs.is_empty() {
            // No connection to reach: no token at all.
            tokens::revoke_in(tx, owner, session.session_id, now)?;
        } else {
            let public = self
                .operator
                .public_url()
                .ok_or_else(|| anyhow!("no public_url to name the gateway's /mcp/ by"))?;
            let token = tokens::mint_in(tx, owner, session.session_id, session.host_id, session.hat_id, now)?;
            for slug in slugs {
                servers.push(McpServer::Http {
                    name: format!("hennery-{slug}"),
                    url: format!("{}/mcp/{slug}", public.origin()),
                    headers: vec![NameValue::new("Authorization", format!("Bearer {}", token.expose()))],
                });
            }
        }
        servers.extend(stdio::delivered_in(
            tx,
            owner,
            session.host_id,
            session.hat_id,
            &self.key,
        )?);
        Ok(Delivered { servers, cut })
    }

    fn revoke_in(&self, tx: &Transaction<'_>, session_id: &str) -> Result<Cut> {
        let owner = self.owner();
        let hash = tokens::session_hash_in(tx, owner, session_id)?;
        let revoked = tokens::revoke_in(tx, owner, session_id, unix_now())?;
        // Only a live token has anything open to end.
        Ok(Cut(hash.filter(|_| revoked).into_iter().collect()))
    }

    fn revoke_host_in(&self, tx: &Transaction<'_>, host_id: &str) -> Result<Cut> {
        let owner = self.owner();
        let cut = Cut(tokens::live_host_hashes_in(tx, owner, host_id)?);
        tokens::revoke_host_in(tx, owner, host_id, unix_now())?;
        Ok(cut)
    }

    fn cut(&self, cut: Cut) {
        self.revocations.cut(cut);
    }

    fn purge_hat(&self, hat_id: &str) -> Result<()> {
        let cut = self.store.purge_hat(hat_id)?;
        self.revocations.cut(cut);
        Ok(())
    }
}
