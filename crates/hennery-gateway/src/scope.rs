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

use crate::model::{CredKind, url_for_logs};
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
    /// connection was changed meanwhile, to another URL or to a `static`
    /// kind without a credential: the answer was about what it was. True if
    /// the status changed.
    pub fn mark_ok(&self, id: &str, url: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE gw_connections SET status = 'ok', status_note = NULL, status_at = ?4
             WHERE id = ?1 AND owner_id = ?2 AND url = ?3 AND status != 'ok'
                 AND (cred_kind = 'none'
                      OR EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = ?1 AND k.owner_id = ?2))",
            params![id, self.owner, url, now],
        )?;
        Ok(changed == 1)
    }

    /// The upstream at `url` refused `id`'s credential, or its lack of one
    /// (401; gateway spec §5.4, §7): `needs_auth`, unless the connection
    /// was changed to another URL meanwhile. True if the status changed.
    pub fn mark_needs_auth(&self, id: &str, url: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE gw_connections SET status = 'needs_auth', status_note = ?4, status_at = ?5
             WHERE id = ?1 AND owner_id = ?2 AND url = ?3 AND status != 'needs_auth'",
            params![id, self.owner, url, "the upstream refused the credential (401)", now],
        )?;
        Ok(changed == 1)
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
        type Row = (String, String, String, String, bool, Option<String>, String);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT c.id, c.label, c.url, c.cred_kind, c.internal_network, c.tool_allowlist, c.status
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
                    ))
                },
            )
            .optional()?;
        let Some((id, label, url, kind, internal_network, allowlist, status)) = row else {
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
        }))
    }
}
