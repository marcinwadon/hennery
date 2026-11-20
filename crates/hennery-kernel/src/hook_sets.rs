//! The hook sets' storage (skills spec §8, §9): typed `hooks` configs, their
//! host assignments, and what a host is delivered at session start.
//! Owner-bound like the rest of `Hosts`.

use crate::hosts::Hosts;
use crate::secret::random_bytes;
use anyhow::Result;
use hennery_proto::frames::HookSetDelivery;
use hennery_proto::hooks::{HookSetConfig, HookSetError, events_of, validate};
use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

/// The most hook sets one owner may store.
pub const MAX_HOOK_SETS: i64 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSetRecord {
    pub id: String,
    pub name: String,
    pub content_hash: String,
    /// The events the config names, derived from it.
    pub events: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Why a hook set write was refused (infrastructure errors are `anyhow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSetWrite {
    /// A hook set with this name exists already.
    Exists,
    /// The owner is at `MAX_HOOK_SETS`.
    TooMany,
    /// The config breaks a limit.
    Invalid(HookSetError),
    /// The name is not a valid catalogue name.
    BadName,
    /// The hook set, or a host to assign it to, is unknown (or revoked).
    Unknown,
}

fn new_id() -> String {
    hex::encode(random_bytes::<16>())
}

/// The config as stored, and its hash: `HookSetConfig` is a `BTreeMap`, so
/// its JSON is canonical.
fn encode(config: &HookSetConfig) -> Result<(String, String)> {
    let json = serde_json::to_vec(config)?;
    let hash = hex::encode(Sha256::digest(&json));
    Ok((String::from_utf8(json)?, hash))
}

fn record(
    id: String,
    name: String,
    hooks: &str,
    content_hash: String,
    created_at: i64,
    updated_at: i64,
) -> Result<HookSetRecord> {
    let config: HookSetConfig = serde_json::from_str(hooks)?;
    Ok(HookSetRecord {
        id,
        name,
        content_hash,
        events: events_of(&config),
        created_at,
        updated_at,
    })
}

/// Drop every hook set assignment of `host_id` inside the caller's
/// transaction (skills spec §10: revoking a host deletes its assignments).
pub(crate) fn drop_host_assignments(tx: &Transaction<'_>, owner: &str, host_id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM hook_set_assignments WHERE owner_id = ?1 AND host_id = ?2",
        [owner, host_id],
    )?;
    Ok(())
}

impl Hosts {
    /// Store a new hook set. `BadName`, `Invalid`, `Exists`, or `TooMany`
    /// at `MAX_HOOK_SETS`.
    pub fn create_hook_set(
        &self,
        name: &str,
        hooks: &HookSetConfig,
        now: i64,
    ) -> Result<Result<HookSetRecord, HookSetWrite>> {
        if !hennery_proto::skills::valid_name(name) {
            return Ok(Err(HookSetWrite::BadName));
        }
        if let Err(e) = validate(hooks) {
            return Ok(Err(HookSetWrite::Invalid(e)));
        }
        let (json, content_hash) = encode(hooks)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hook_sets WHERE owner_id = ?1 AND name = ?2)",
            params![self.owner, name],
            |r| r.get(0),
        )?;
        if taken {
            return Ok(Err(HookSetWrite::Exists));
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM hook_sets WHERE owner_id = ?1",
            [&self.owner],
            |r| r.get(0),
        )?;
        if count >= MAX_HOOK_SETS {
            return Ok(Err(HookSetWrite::TooMany));
        }
        let id = new_id();
        tx.execute(
            "INSERT INTO hook_sets(id, owner_id, name, hooks, content_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, self.owner, name, json, content_hash, now],
        )?;
        tx.commit()?;
        Ok(Ok(record(id, name.to_string(), &json, content_hash, now, now)?))
    }

    /// Replace a hook set's config. `Invalid` for a config that breaks a
    /// limit, `Unknown` when there is no such hook set.
    pub fn replace_hook_set(
        &self,
        id: &str,
        hooks: &HookSetConfig,
        now: i64,
    ) -> Result<Result<HookSetRecord, HookSetWrite>> {
        if let Err(e) = validate(hooks) {
            return Ok(Err(HookSetWrite::Invalid(e)));
        }
        let (json, content_hash) = encode(hooks)?;
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let current: Option<(String, i64)> = tx
            .query_row(
                "SELECT name, created_at FROM hook_sets WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((name, created_at)) = current else {
            return Ok(Err(HookSetWrite::Unknown));
        };
        tx.execute(
            "UPDATE hook_sets SET hooks = ?1, content_hash = ?2, updated_at = ?3 WHERE id = ?4 AND owner_id = ?5",
            params![json, content_hash, now, id, self.owner],
        )?;
        tx.commit()?;
        Ok(Ok(record(id.to_string(), name, &json, content_hash, created_at, now)?))
    }

    /// Delete a hook set and, by cascade, its assignments. False when unknown.
    pub fn delete_hook_set(&self, id: &str) -> Result<bool> {
        let conn = self.conn();
        let deleted = conn.execute(
            "DELETE FROM hook_sets WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner],
        )?;
        Ok(deleted > 0)
    }

    /// The owner's hook sets, by name.
    pub fn list_hook_sets(&self) -> Result<Vec<HookSetRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, hooks, content_hash, created_at, updated_at FROM hook_sets
             WHERE owner_id = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map([&self.owner], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, String>(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, name, hooks, hash, created, updated)| record(id, name, &hooks, hash, created, updated))
            .collect()
    }

    /// One hook set and its config.
    pub fn hook_set(&self, id: &str) -> Result<Option<(HookSetRecord, HookSetConfig)>> {
        let conn = self.conn();
        let row: Option<(String, String, String, String, i64, i64)> = conn
            .query_row(
                "SELECT id, name, hooks, content_hash, created_at, updated_at FROM hook_sets
                 WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        row.map(|(id, name, hooks, hash, created, updated)| {
            Ok((
                record(id, name, &hooks, hash, created, updated)?,
                serde_json::from_str(&hooks)?,
            ))
        })
        .transpose()
    }

    /// Replace the set of hosts a hook set is assigned to. `Unknown` when
    /// the hook set, or any host, is unknown or revoked; nothing changes then.
    pub fn set_hook_set_hosts(&self, id: &str, host_ids: &[String], now: i64) -> Result<Result<(), HookSetWrite>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM hook_sets WHERE id = ?1 AND owner_id = ?2)",
            params![id, self.owner],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(Err(HookSetWrite::Unknown));
        }
        for host in host_ids {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1 AND owner_id = ?2 AND revoked_at IS NULL)",
                params![host, self.owner],
                |r| r.get(0),
            )?;
            if !live {
                return Ok(Err(HookSetWrite::Unknown));
            }
        }
        tx.execute(
            "DELETE FROM hook_set_assignments WHERE owner_id = ?1 AND hook_set_id = ?2",
            params![self.owner, id],
        )?;
        for host in host_ids {
            tx.execute(
                "INSERT OR IGNORE INTO hook_set_assignments(owner_id, hook_set_id, host_id, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![self.owner, id, host, now],
            )?;
        }
        tx.commit()?;
        Ok(Ok(()))
    }

    /// The hosts a hook set is assigned to, by id.
    pub fn hook_set_hosts(&self, id: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT host_id FROM hook_set_assignments WHERE owner_id = ?1 AND hook_set_id = ?2 ORDER BY host_id",
        )?;
        let rows = stmt.query_map(params![self.owner, id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// What `host_id` is delivered at session start: its hook sets, by name.
    pub fn hook_sets_for_host(&self, host_id: &str) -> Result<Vec<HookSetDelivery>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.content_hash, s.hooks FROM hook_set_assignments a
             JOIN hook_sets s ON s.id = a.hook_set_id AND s.owner_id = a.owner_id
             WHERE a.owner_id = ?1 AND a.host_id = ?2 ORDER BY s.name",
        )?;
        let rows = stmt
            .query_map(params![self.owner, host_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, name, content_hash, hooks)| {
                Ok(HookSetDelivery {
                    id,
                    name,
                    content_hash,
                    hooks: serde_json::from_str(&hooks)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::Enrollment;
    use ed25519_dalek::SigningKey;

    const NOW: i64 = 1_800_000_000;

    fn config(event: &str, command: &str) -> HookSetConfig {
        serde_json::from_value(serde_json::json!({
            event: [{"matcher": "Bash", "hooks": [{"type": "command", "command": command}]}]
        }))
        .unwrap()
    }

    fn host(hosts: &Hosts, id: &str, seed: u8) {
        let enrollment = Enrollment {
            public_key: hex::encode(SigningKey::from_bytes(&[seed; 32]).verifying_key().as_bytes()),
            name: "laptop".into(),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        hosts.register(id, &enrollment, NOW).unwrap();
    }

    fn create(hosts: &Hosts, name: &str, command: &str) -> HookSetRecord {
        hosts
            .create_hook_set(name, &config("PreToolUse", command), NOW)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn create_list_replace_delete_round_trip() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-a", 1);
        let b = create(&hosts, "b", "/bin/b");
        let a = create(&hosts, "a", "/bin/a");
        assert_eq!(a.events, ["PreToolUse"]);
        assert_eq!(a.content_hash.len(), 64);
        let names: Vec<_> = hosts.list_hook_sets().unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "b"]);
        let (got, cfg) = hosts.hook_set(&a.id).unwrap().unwrap();
        assert_eq!((got, cfg), (a.clone(), config("PreToolUse", "/bin/a")));

        let replaced = hosts
            .replace_hook_set(&b.id, &config("PostToolUse", "/bin/b2"), NOW + 5)
            .unwrap()
            .unwrap();
        assert_ne!(replaced.content_hash, b.content_hash);
        assert_eq!(replaced.events, ["PostToolUse"]);
        assert_eq!((replaced.created_at, replaced.updated_at), (NOW, NOW + 5));
        assert_eq!(
            hosts
                .replace_hook_set("nope", &config("PreToolUse", "x"), NOW)
                .unwrap()
                .unwrap_err(),
            HookSetWrite::Unknown
        );

        hosts
            .set_hook_set_hosts(&a.id, &["host-a".into()], NOW)
            .unwrap()
            .unwrap();
        assert_eq!(hosts.hook_set_hosts(&a.id).unwrap(), ["host-a"]);
        assert!(hosts.delete_hook_set(&a.id).unwrap());
        assert!(!hosts.delete_hook_set(&a.id).unwrap());
        assert!(hosts.hook_set(&a.id).unwrap().is_none());
        assert!(hosts.hook_set_hosts(&a.id).unwrap().is_empty());
        assert!(hosts.hook_sets_for_host("host-a").unwrap().is_empty());
    }

    #[test]
    fn a_duplicate_name_is_refused() {
        let hosts = Hosts::open_in_memory().unwrap();
        create(&hosts, "lint", "/bin/a");
        let again = hosts
            .create_hook_set("lint", &config("PreToolUse", "/bin/b"), NOW)
            .unwrap();
        assert_eq!(again.unwrap_err(), HookSetWrite::Exists);
    }

    #[test]
    fn a_bad_name_is_refused() {
        let hosts = Hosts::open_in_memory().unwrap();
        let got = hosts
            .create_hook_set("Bad Name", &config("PreToolUse", "x"), NOW)
            .unwrap();
        assert_eq!(got.unwrap_err(), HookSetWrite::BadName);
    }

    /// The typed config cannot hold a non-command handler (the API layer
    /// refuses it at parse time); a parsed command handler under an unknown
    /// event is still refused here.
    #[test]
    fn a_non_command_handler_is_refused() {
        let hosts = Hosts::open_in_memory().unwrap();
        let parsed: HookSetConfig =
            serde_json::from_str(r#"{"NotAnEvent":[{"hooks":[{"type":"command","command":"/bin/x"}]}]}"#).unwrap();
        let got = hosts.create_hook_set("x", &parsed, NOW).unwrap();
        assert_eq!(
            got.unwrap_err(),
            HookSetWrite::Invalid(HookSetError::UnknownEvent("NotAnEvent".into()))
        );
        assert!(
            serde_json::from_str::<HookSetConfig>(r#"{"PreToolUse":[{"hooks":[{"type":"http","command":"x"}]}]}"#)
                .is_err()
        );
    }

    #[test]
    fn an_unknown_event_is_refused() {
        let hosts = Hosts::open_in_memory().unwrap();
        let got = hosts.create_hook_set("x", &config("Bogus", "/bin/x"), NOW).unwrap();
        assert_eq!(
            got.unwrap_err(),
            HookSetWrite::Invalid(HookSetError::UnknownEvent("Bogus".into()))
        );
        let a = create(&hosts, "a", "/bin/a");
        let got = hosts.replace_hook_set(&a.id, &config("Bogus", "/bin/x"), NOW).unwrap();
        assert!(matches!(got.unwrap_err(), HookSetWrite::Invalid(_)));
        assert_eq!(hosts.list_hook_sets().unwrap().len(), 1);
    }

    #[test]
    fn the_owner_cap_is_64() {
        let hosts = Hosts::open_in_memory().unwrap();
        for i in 0..MAX_HOOK_SETS {
            create(&hosts, &format!("set-{i}"), "/bin/x");
        }
        let over = hosts
            .create_hook_set("one-more", &config("PreToolUse", "/bin/x"), NOW)
            .unwrap();
        assert_eq!(over.unwrap_err(), HookSetWrite::TooMany);
    }

    #[test]
    fn hosts_are_assigned_and_delivered_in_name_order() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-a", 1);
        host(&hosts, "host-b", 2);
        let b = create(&hosts, "b", "/bin/b");
        let a = create(&hosts, "a", "/bin/a");
        create(&hosts, "c", "/bin/c");
        hosts
            .set_hook_set_hosts(&b.id, &["host-a".into()], NOW)
            .unwrap()
            .unwrap();
        hosts
            .set_hook_set_hosts(&a.id, &["host-a".into(), "host-b".into()], NOW)
            .unwrap()
            .unwrap();
        let delivered = hosts.hook_sets_for_host("host-a").unwrap();
        assert_eq!(
            delivered,
            [
                HookSetDelivery {
                    id: a.id.clone(),
                    name: "a".into(),
                    content_hash: a.content_hash.clone(),
                    hooks: config("PreToolUse", "/bin/a"),
                },
                HookSetDelivery {
                    id: b.id.clone(),
                    name: "b".into(),
                    content_hash: b.content_hash.clone(),
                    hooks: config("PreToolUse", "/bin/b"),
                },
            ]
        );
        assert_eq!(hosts.hook_sets_for_host("host-b").unwrap().len(), 1);
        assert_eq!(hosts.hook_set_hosts(&a.id).unwrap(), ["host-a", "host-b"]);
        // Setting again replaces the set.
        hosts.set_hook_set_hosts(&a.id, &[], NOW).unwrap().unwrap();
        assert!(hosts.hook_sets_for_host("host-b").unwrap().is_empty());
    }

    #[test]
    fn an_unknown_or_revoked_host_cannot_be_assigned() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        host(&hosts, "host-2", 2);
        let a = create(&hosts, "a", "/bin/a");
        hosts.revoke("host-2", NOW).unwrap();
        for bad in [vec!["ghost".to_string()], vec!["host-1".into(), "host-2".into()]] {
            assert_eq!(
                hosts.set_hook_set_hosts(&a.id, &bad, NOW).unwrap().unwrap_err(),
                HookSetWrite::Unknown
            );
        }
        assert_eq!(
            hosts
                .set_hook_set_hosts("nope", &["host-1".into()], NOW)
                .unwrap()
                .unwrap_err(),
            HookSetWrite::Unknown
        );
        assert!(hosts.hook_set_hosts(&a.id).unwrap().is_empty());
    }

    #[test]
    fn revoking_a_host_drops_its_hook_set_assignments() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        host(&hosts, "host-2", 2);
        let a = create(&hosts, "a", "/bin/a");
        hosts
            .set_hook_set_hosts(&a.id, &["host-1".into(), "host-2".into()], NOW)
            .unwrap()
            .unwrap();
        hosts.revoke("host-2", NOW).unwrap();
        assert_eq!(hosts.hook_set_hosts(&a.id).unwrap(), ["host-1"]);
        assert!(hosts.hook_sets_for_host("host-2").unwrap().is_empty());
        // The set can still be re-assigned.
        hosts
            .set_hook_set_hosts(&a.id, &["host-1".into()], NOW)
            .unwrap()
            .unwrap();
    }
}
