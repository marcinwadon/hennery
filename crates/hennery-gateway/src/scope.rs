//! Who presents a token, and what it may reach (gateway spec §3.1,
//! umbrella §10.2): `ClientIdentity` turns a token into a `Principal`,
//! `MountPolicy` decides whether a connection is in that principal's
//! scope. Both are checked at every request, from the token and the mounts
//! as they are then, never from anything the request claims.
//!
//! Full mode: `ProxyStore` implements both. A session token resolves to
//! "session S on host X, hat H"; its connections are those of hat H
//! mounted on host X, a host that is not revoked. Standalone mode (plan
//! 8g) adds a client principal, resolved from `gw_clients`, whose
//! connections are its pins.
//!
//! Every SQL statement of this file names the owner, and the owner audit
//! reads it (`hennery-testkit/tests/owner_filter.rs`).

use crate::model::{CredKind, Status, StatusChange, url_for_logs};
use crate::schema::{COMPONENT, MIGRATIONS};
use crate::tokens::{is_session_token, token_hash};
use anyhow::{Context, Result, anyhow};
use hennery_kernel::db;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// How stale `last_used_at` may get before a request writes it again, in
/// seconds (plan 8d decision 12): one write a minute per session at most,
/// not one per request.
pub const LAST_USED_EVERY: i64 = 60;

/// Who presented a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// The hat whose connections it may reach, and no other's.
    pub hat_id: String,
    pub kind: PrincipalKind,
}

/// The kinds of principal (gateway spec §3.1). Plan 8g adds the standalone
/// client, scoped by its pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrincipalKind {
    /// A hennery session: its connections are those of its hat mounted on
    /// its host.
    Session { session_id: String, host_id: String },
}

/// Token → principal (umbrella §10.2).
pub trait ClientIdentity: Send + Sync + 'static {
    /// The principal `token` names now, or `None`: unknown, revoked,
    /// superseded, or of a revoked host. Each is the same 404 to the client.
    fn resolve(&self, token: &str, now: i64) -> Result<Option<Principal>>;
}

/// Principal → connections (umbrella §10.2).
pub trait MountPolicy: Send + Sync + 'static {
    /// The connection with `slug` if it is in `principal`'s scope now;
    /// `None` for one that is not, or does not exist (the same 404).
    fn connection(&self, principal: &Principal, slug: &str) -> Result<Option<ScopedConnection>>;
}

/// A connection as the proxy needs it, read in scope. Where a `static`
/// connection's token goes is read again with the token itself
/// (`GatewayStore::static_credential`, the review's R2 of plan 8a).
#[derive(Clone, PartialEq, Eq)]
pub struct ScopedConnection {
    pub id: String,
    pub slug: String,
    pub label: String,
    pub url: String,
    pub cred_kind: CredKind,
    pub internal_network: bool,
    /// `None`: every tool.
    pub tool_allowlist: Option<Vec<String>>,
    /// `not_connected`, `ok`, `needs_auth` or `error`.
    pub status: String,
    /// Whether a credential is stored, as of the read.
    pub has_credential: bool,
}

// `Debug` by hand: the URL shows only its origin (lane L11).
impl std::fmt::Debug for ScopedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedConnection")
            .field("id", &self.id)
            .field("slug", &self.slug)
            .field("label", &self.label)
            .field("url", &url_for_logs(&self.url))
            .field("cred_kind", &self.cred_kind)
            .field("internal_network", &self.internal_network)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("status", &self.status)
            .finish()
    }
}

/// The proxy's reads and writes of `hennery.db`, on a connection of its
/// own: session tokens resolved, connections in scope, and what live
/// traffic says of a connection's status (gateway spec §7).
pub struct ProxyStore {
    conn: Mutex<Connection>,
    owner: String,
}

impl ProxyStore {
    /// Open on `hennery.db`, migrating the kernel's tables and the
    /// gateway's first, as `GatewayStore::open` does.
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = db::open(path)?;
        let owner = db::kernel_owner(&mut conn)?;
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self {
            conn: Mutex::new(conn),
            owner,
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("proxy store lock")
    }

    pub fn owner_id(&self) -> &str {
        &self.owner
    }

    /// Live traffic got a 2xx through `id` with its URL `url` (gateway spec
    /// §7): its status becomes `ok` if it was anything else. Not if the
    /// connection was changed meanwhile, to another URL or to a kind
    /// without a credential: the answer was about what it was. `checked_at`
    /// moves at most every `LAST_USED_EVERY` seconds (plan 8d's hand-off),
    /// not on every request. The transition, if there was one.
    pub fn record_traffic_ok(&self, id: &str, url: &str, now: i64) -> Result<Option<StatusChange>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let row: Option<(String, String, String, Option<i64>)> = tx
            .query_row(
                "SELECT status, label, hat_id, checked_at FROM gw_connections
                 WHERE id = ?1 AND owner_id = ?2 AND url = ?3
                     AND (cred_kind = 'none'
                          OR EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = ?1 AND k.owner_id = ?2))",
                params![id, self.owner, url],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((status, label, hat_id, checked_at)) = row else {
            return Ok(None);
        };
        let from = Status::parse(&status).ok_or_else(|| anyhow!("a stored status"))?;
        if from == Status::Ok {
            if checked_at.is_none_or(|at| now - at >= LAST_USED_EVERY) {
                tx.execute(
                    "UPDATE gw_connections SET checked_at = ?3 WHERE id = ?1 AND owner_id = ?2",
                    params![id, self.owner, now],
                )?;
                tx.commit()?;
            }
            return Ok(None);
        }
        tx.execute(
            "UPDATE gw_connections SET status = 'ok', status_note = NULL, status_at = ?3, checked_at = ?3
             WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner, now],
        )?;
        tx.commit()?;
        Ok(Some(StatusChange {
            connection_id: id.to_string(),
            hat_id,
            label,
            from,
            to: Status::Ok,
        }))
    }

    /// `id`'s status is `to` as of `now`, with `note`, unless the
    /// connection was changed to another URL than `url` meanwhile (gateway
    /// spec §5.4, §7): the upstream refused its credential (`needs_auth`),
    /// or the probe says `ok` or `error`. `checked_at` moves either way.
    /// The transition, if there was one.
    pub fn record_status(
        &self,
        id: &str,
        url: &str,
        to: Status,
        note: Option<&str>,
        now: i64,
    ) -> Result<Option<StatusChange>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let row: Option<(String, String, String)> = tx
            .query_row(
                "SELECT status, label, hat_id FROM gw_connections WHERE id = ?1 AND owner_id = ?2 AND url = ?3",
                params![id, self.owner, url],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((status, label, hat_id)) = row else {
            return Ok(None);
        };
        let from = Status::parse(&status).ok_or_else(|| anyhow!("a stored status"))?;
        tx.execute(
            "UPDATE gw_connections SET status = ?3, status_note = ?4, checked_at = ?5,
                 status_at = CASE WHEN status = ?3 THEN status_at ELSE ?5 END
             WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner, to.as_str(), note, now],
        )?;
        tx.commit()?;
        Ok((from != to).then(|| StatusChange {
            connection_id: id.to_string(),
            hat_id,
            label,
            from,
            to,
        }))
    }

    /// The upstream at `url` refused `id`'s credential, or its lack of one
    /// (401; gateway spec §5.4, §7): `needs_auth`, with `note`.
    pub fn mark_needs_auth(&self, id: &str, url: &str, note: &str, now: i64) -> Result<Option<StatusChange>> {
        self.record_status(id, url, Status::NeedsAuth, Some(note), now)
    }

    /// Only `checked_at` moves: the probe learned nothing that changes the
    /// status (another 4xx: gateway spec §7), while the URL is `url`.
    pub fn record_checked(&self, id: &str, url: &str, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE gw_connections SET checked_at = ?4 WHERE id = ?1 AND owner_id = ?2 AND url = ?3",
            params![id, self.owner, url, now],
        )?;
        Ok(())
    }

    /// Every connection in `needs_auth` or `error` now, as the transition
    /// a startup announces once (gateway spec §7).
    pub fn problems(&self) -> Result<Vec<StatusChange>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, hat_id, label, status FROM gw_connections
             WHERE owner_id = ?1 AND status IN ('needs_auth', 'error') ORDER BY created_at, id",
        )?;
        let rows = stmt.query_map([&self.owner], |r| {
            Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (connection_id, hat_id, label, status) = row?;
            out.push(StatusChange {
                connection_id,
                hat_id,
                label,
                from: Status::Ok,
                to: Status::parse(&status).ok_or_else(|| anyhow!("a stored status"))?,
            });
        }
        Ok(out)
    }

    /// The connections the background probe checks (gateway spec §7): the
    /// OAuth ones with a grant. Static and `none` connections are not
    /// probed, nor any without a credential.
    pub fn probe_targets(&self) -> Result<Vec<ScopedConnection>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT c.id FROM gw_connections c
             WHERE c.owner_id = ?1 AND c.cred_kind IN ('oauth_dcr', 'oauth_client')
                 AND EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1)
             ORDER BY c.created_at, c.id",
        )?;
        let ids = stmt
            .query_map([&self.owner], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        drop(conn);
        let mut out = Vec::new();
        for id in ids {
            if let Some(connection) = self.connection_by_id(&id)? {
                out.push(connection);
            }
        }
        Ok(out)
    }

    /// The connection `id` as the proxy reads one, whatever its mounts: for
    /// a probe, which acts for the operator, not for a principal. With
    /// whether it has a credential.
    pub fn connection_by_id(&self, id: &str) -> Result<Option<ScopedConnection>> {
        type Row = (String, String, String, String, bool, Option<String>, String, bool);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT c.slug, c.label, c.url, c.cred_kind, c.internal_network, c.tool_allowlist, c.status,
                        EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1)
                 FROM gw_connections c WHERE c.owner_id = ?1 AND c.id = ?2",
                params![self.owner, id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((slug, label, url, kind, internal_network, allowlist, status, has_credential)) = row else {
            return Ok(None);
        };
        Ok(Some(ScopedConnection {
            cred_kind: CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?,
            tool_allowlist: allowlist
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .context("a stored tool allowlist")?,
            id: id.to_string(),
            slug,
            label,
            url,
            internal_network,
            status,
            has_credential,
        }))
    }
}

impl ClientIdentity for ProxyStore {
    fn resolve(&self, token: &str, now: i64) -> Result<Option<Principal>> {
        if !is_session_token(token) {
            return Ok(None);
        }
        let hash = token_hash(token);
        let conn = self.conn();
        let row: Option<(String, String, String, Option<i64>)> = conn
            .query_row(
                "SELECT t.session_id, t.host_id, t.hat_id, t.last_used_at
                 FROM gw_session_tokens t JOIN hosts h ON h.id = t.host_id AND h.owner_id = ?1
                 WHERE t.owner_id = ?1 AND t.token_hash = ?2 AND t.revoked_at IS NULL AND h.revoked_at IS NULL",
                params![self.owner, hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((session_id, host_id, hat_id, last_used_at)) = row else {
            return Ok(None);
        };
        if last_used_at.is_none_or(|at| now - at >= LAST_USED_EVERY) {
            // `token_hash` too: only the row just read is stamped, never one
            // a mint superseded in between.
            conn.execute(
                "UPDATE gw_session_tokens SET last_used_at = ?3
                 WHERE session_id = ?1 AND owner_id = ?2 AND token_hash = ?4",
                params![session_id, self.owner, now, hash],
            )?;
        }
        Ok(Some(Principal {
            hat_id,
            kind: PrincipalKind::Session { session_id, host_id },
        }))
    }
}

impl MountPolicy for ProxyStore {
    fn connection(&self, principal: &Principal, slug: &str) -> Result<Option<ScopedConnection>> {
        let PrincipalKind::Session { host_id, .. } = &principal.kind;
        type Row = (String, String, String, String, bool, Option<String>, String, bool);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT c.id, c.label, c.url, c.cred_kind, c.internal_network, c.tool_allowlist, c.status,
                        EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1)
                 FROM gw_connections c
                 WHERE c.owner_id = ?1 AND c.slug = ?2 AND c.hat_id = ?3
                     AND EXISTS (SELECT 1 FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
                                 WHERE m.connection_id = c.id AND m.owner_id = ?1 AND m.host_id = ?4
                                     AND h.revoked_at IS NULL)",
                params![self.owner, slug, principal.hat_id, host_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, label, url, kind, internal_network, allowlist, status, has_credential)) = row else {
            return Ok(None);
        };
        Ok(Some(ScopedConnection {
            cred_kind: CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?,
            tool_allowlist: allowlist
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .context("a stored tool allowlist")?,
            id,
            slug: slug.to_string(),
            label,
            url,
            internal_network,
            status,
            has_credential,
        }))
    }
}
