//! Project recents (kernel spec §1.1, §5.3; plan 6c decision 11): the
//! directories sessions started or resumed in, per host and hat, newest
//! first. They share the host registry's connection and owner, and every
//! query names the owner (kernel spec §1).

use crate::hats::{is_canonical, resolve};
use crate::hosts::{Hosts, is_displayable_path};
use anyhow::Result;
use rusqlite::{TransactionBehavior, params};
use std::collections::HashSet;

/// The recents kept per host and hat; older ones are dropped.
pub const KEPT: usize = 50;

/// The recents the project picker shows.
pub const SHOWN: usize = 20;

/// One recent project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    pub path: String,
    /// Seconds since the Unix epoch.
    pub last_used_at: i64,
}

impl Hosts {
    /// Remember that a session of `hat_id` started or resumed in `path` on
    /// `host_id` at `now`, keeping the newest `KEPT` of that host and hat.
    /// `false`, and nothing remembered, for a path that is not canonical or
    /// cannot be shown: until the host resolves a session's cwd (hats 5b),
    /// it is what its client sent.
    pub fn remember(&self, host_id: &str, hat_id: &str, path: &str, now: i64) -> Result<bool> {
        if !is_canonical(path) || !is_displayable_path(path) {
            return Ok(false);
        }
        let owner = self.owner_id();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO project_recents(owner_id, host_id, hat_id, path, last_used_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(host_id, hat_id, path) DO UPDATE SET last_used_at = excluded.last_used_at
                 WHERE project_recents.owner_id = ?1",
            params![owner, host_id, hat_id, path, now],
        )?;
        tx.execute(
            "DELETE FROM project_recents WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3 AND path NOT IN (
                 SELECT path FROM project_recents WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3
                 ORDER BY last_used_at DESC, path LIMIT ?4)",
            params![owner, host_id, hat_id, KEPT as i64],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The newest `limit` recents of `host_id` whose path resolves to
    /// `hat_id` now (kernel spec §5.3; the review's A1): resolved against
    /// the host's current rules and default hat, not the hat stored when
    /// each was remembered, which a rule or a default changed since leaves
    /// stale. Empty for an unknown host.
    pub fn recents(&self, host_id: &str, hat_id: &str, limit: usize) -> Result<Vec<Recent>> {
        let (Some(rules), Some(host)) = (self.path_rules(host_id)?, self.host(host_id)?) else {
            return Ok(Vec::new());
        };
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT path, last_used_at FROM project_recents WHERE owner_id = ?1 AND host_id = ?2
             ORDER BY last_used_at DESC, path",
        )?;
        let rows = stmt.query_map(params![self.owner_id(), host_id], |r| {
            Ok(Recent {
                path: r.get(0)?,
                last_used_at: r.get(1)?,
            })
        })?;
        let mut seen = HashSet::new();
        let mut shown = Vec::new();
        for recent in rows {
            let recent = recent?;
            // Newest first, so a path stored under two hats is shown once,
            // with its latest use.
            if resolve(&rules, &host.default_hat_id, &recent.path).hat_id == hat_id && seen.insert(recent.path.clone())
            {
                shown.push(recent);
                if shown.len() == limit {
                    break;
                }
            }
        }
        Ok(shown)
    }
}
