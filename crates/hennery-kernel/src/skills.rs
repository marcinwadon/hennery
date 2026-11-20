//! The skills catalogue's storage (skills spec §5): skills, their host
//! assignments, the per-owner revision that versions the assignment set,
//! and what each host last reported. Owner-bound like the rest of `Hosts`.

use crate::hosts::Hosts;
use crate::secret::random_bytes;
use crate::skill_archive::{Inspected, MAX_FILE};
use anyhow::Result;
use hennery_proto::frames::{SkillFailed, SkillSyncItem};
use rusqlite::{OptionalExtension, Transaction, params};

/// The most skills one owner may store.
pub const MAX_SKILLS: i64 = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content_hash: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Why a skill write was refused (infrastructure errors are `anyhow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillWrite {
    /// A skill with this name exists already.
    Exists,
    /// The owner is at `MAX_SKILLS`.
    TooMany,
    /// A replacement archive names a different skill.
    NameMismatch,
    /// The skill, or a host to assign it to, is unknown (or revoked).
    Unknown,
}

/// What a host last reported about applying the skills (skills spec §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillHostReport {
    pub revision: u64,
    pub applied: Vec<String>,
    pub failed: Vec<SkillFailed>,
    pub reported_at: i64,
}

const COLUMNS: &str = "id, name, description, content_hash, created_at, updated_at";

fn read_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<SkillRecord> {
    Ok(SkillRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        content_hash: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
    })
}

fn new_id() -> String {
    hex::encode(random_bytes::<16>())
}

/// Bump the owner's revision inside the caller's transaction.
fn bump(tx: &Transaction<'_>, owner: &str) -> Result<u64> {
    let revision: i64 = tx.query_row(
        "INSERT INTO skill_revisions(owner_id, revision) VALUES (?1, 1)
         ON CONFLICT(owner_id) DO UPDATE SET revision = revision + 1 RETURNING revision",
        [owner],
        |r| r.get(0),
    )?;
    Ok(revision as u64)
}

/// Drop every skill assignment of `host_id` inside the caller's
/// transaction (skills spec §10: revoking a host deletes its assignments),
/// bumping the owner's revision when there were any.
pub(crate) fn drop_host_assignments(tx: &Transaction<'_>, owner: &str, host_id: &str) -> Result<()> {
    let dropped = tx.execute(
        "DELETE FROM skill_assignments WHERE owner_id = ?1 AND host_id = ?2",
        [owner, host_id],
    )?;
    if dropped > 0 {
        bump(tx, owner)?;
    }
    Ok(())
}

impl Hosts {
    /// Store a new skill from an inspected archive. `Exists` when the name
    /// is taken, `TooMany` at `MAX_SKILLS`.
    pub fn create_skill(
        &self,
        inspected: &Inspected,
        archive: &[u8],
        now: i64,
    ) -> Result<Result<SkillRecord, SkillWrite>> {
        debug_assert!(inspected.skill_md.len() as u64 <= MAX_FILE);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let name = &inspected.frontmatter.name;
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM skill_items WHERE owner_id = ?1 AND name = ?2)",
            params![self.owner, name],
            |r| r.get(0),
        )?;
        if taken {
            return Ok(Err(SkillWrite::Exists));
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM skill_items WHERE owner_id = ?1",
            [&self.owner],
            |r| r.get(0),
        )?;
        if count >= MAX_SKILLS {
            return Ok(Err(SkillWrite::TooMany));
        }
        let record = SkillRecord {
            id: new_id(),
            name: name.clone(),
            description: inspected.frontmatter.description.clone(),
            content_hash: inspected.content_hash.clone(),
            created_at: now,
            updated_at: now,
        };
        tx.execute(
            "INSERT INTO skill_items(id, owner_id, name, description, skill_md, content_hash, archive, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                record.id,
                self.owner,
                record.name,
                record.description,
                inspected.skill_md,
                record.content_hash,
                archive,
                now
            ],
        )?;
        bump(&tx, &self.owner)?;
        tx.commit()?;
        Ok(Ok(record))
    }

    /// Replace a skill's archive. `NameMismatch` when the new `SKILL.md`
    /// names a different skill, `Unknown` when there is no such skill.
    pub fn replace_skill_archive(
        &self,
        id: &str,
        inspected: &Inspected,
        archive: &[u8],
        now: i64,
    ) -> Result<Result<SkillRecord, SkillWrite>> {
        debug_assert!(inspected.skill_md.len() as u64 <= MAX_FILE);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let current: Option<SkillRecord> = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM skill_items WHERE id = ?1 AND owner_id = ?2"),
                params![id, self.owner],
                read_record,
            )
            .optional()?;
        let Some(mut record) = current else {
            return Ok(Err(SkillWrite::Unknown));
        };
        if record.name != inspected.frontmatter.name {
            return Ok(Err(SkillWrite::NameMismatch));
        }
        record.description = inspected.frontmatter.description.clone();
        record.content_hash = inspected.content_hash.clone();
        record.updated_at = now;
        tx.execute(
            "UPDATE skill_items SET description = ?1, skill_md = ?2, content_hash = ?3, archive = ?4, updated_at = ?5
             WHERE id = ?6 AND owner_id = ?7",
            params![
                record.description,
                inspected.skill_md,
                record.content_hash,
                archive,
                now,
                id,
                self.owner
            ],
        )?;
        bump(&tx, &self.owner)?;
        tx.commit()?;
        Ok(Ok(record))
    }

    /// Delete a skill and, by cascade, its assignments. False when unknown.
    pub fn delete_skill(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let deleted = tx.execute(
            "DELETE FROM skill_items WHERE id = ?1 AND owner_id = ?2",
            params![id, self.owner],
        )?;
        if deleted == 0 {
            return Ok(false);
        }
        bump(&tx, &self.owner)?;
        tx.commit()?;
        Ok(true)
    }

    /// The owner's skills, by name.
    pub fn list_skills(&self) -> Result<Vec<SkillRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM skill_items WHERE owner_id = ?1 ORDER BY name"
        ))?;
        let rows = stmt.query_map([&self.owner], read_record)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn skill(&self, id: &str) -> Result<Option<SkillRecord>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM skill_items WHERE id = ?1 AND owner_id = ?2"),
                params![id, self.owner],
                read_record,
            )
            .optional()?)
    }

    pub fn skill_md(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                "SELECT skill_md FROM skill_items WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The stored archive: its hash and bytes.
    pub fn skill_archive(&self, id: &str) -> Result<Option<(String, Vec<u8>)>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                "SELECT content_hash, archive FROM skill_items WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Replace the set of hosts a skill is assigned to. `Unknown` when the
    /// skill, or any host, is unknown or revoked; nothing changes then.
    pub fn set_skill_hosts(&self, id: &str, host_ids: &[String], now: i64) -> Result<Result<(), SkillWrite>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM skill_items WHERE id = ?1 AND owner_id = ?2)",
            params![id, self.owner],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(Err(SkillWrite::Unknown));
        }
        for host in host_ids {
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM hosts WHERE id = ?1 AND owner_id = ?2 AND revoked_at IS NULL)",
                params![host, self.owner],
                |r| r.get(0),
            )?;
            if !live {
                return Ok(Err(SkillWrite::Unknown));
            }
        }
        tx.execute(
            "DELETE FROM skill_assignments WHERE owner_id = ?1 AND skill_id = ?2",
            params![self.owner, id],
        )?;
        for host in host_ids {
            tx.execute(
                "INSERT OR IGNORE INTO skill_assignments(owner_id, skill_id, host_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![self.owner, id, host, now],
            )?;
        }
        bump(&tx, &self.owner)?;
        tx.commit()?;
        Ok(Ok(()))
    }

    /// The hosts a skill is assigned to, by id.
    pub fn skill_hosts(&self, id: &str) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT host_id FROM skill_assignments WHERE owner_id = ?1 AND skill_id = ?2 ORDER BY host_id")?;
        let rows = stmt.query_map(params![self.owner, id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The owner's skills revision: 0 until the first write.
    pub fn skills_revision(&self) -> Result<u64> {
        let conn = self.conn();
        revision(&conn, &self.owner)
    }

    /// The revision and what `host_id` should have, by name.
    pub fn skill_sync_for_host(&self, host_id: &str) -> Result<(u64, Vec<SkillSyncItem>)> {
        let conn = self.conn();
        let revision = revision(&conn, &self.owner)?;
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.content_hash FROM skill_assignments a
             JOIN skill_items s ON s.id = a.skill_id AND s.owner_id = a.owner_id
             WHERE a.owner_id = ?1 AND a.host_id = ?2 ORDER BY s.name",
        )?;
        let items = stmt
            .query_map(params![self.owner, host_id], |r| {
                Ok(SkillSyncItem {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    content_hash: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok((revision, items))
    }

    /// Keep what `host_id` reported, replacing its last report.
    pub fn record_skill_state(&self, host_id: &str, report: &SkillHostReport) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO skill_host_states(owner_id, host_id, revision, applied, failed, reported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(owner_id, host_id) DO UPDATE SET revision = excluded.revision,
                applied = excluded.applied, failed = excluded.failed, reported_at = excluded.reported_at",
            params![
                self.owner,
                host_id,
                report.revision as i64,
                serde_json::to_string(&report.applied)?,
                serde_json::to_string(&report.failed)?,
                report.reported_at
            ],
        )?;
        Ok(())
    }

    pub fn skill_state(&self, host_id: &str) -> Result<Option<SkillHostReport>> {
        let conn = self.conn();
        let row: Option<(i64, String, String, i64)> = conn
            .query_row(
                "SELECT revision, applied, failed, reported_at FROM skill_host_states WHERE owner_id = ?1 AND host_id = ?2",
                params![self.owner, host_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        row.map(|(revision, applied, failed, reported_at)| {
            Ok(SkillHostReport {
                revision: revision as u64,
                applied: serde_json::from_str(&applied)?,
                failed: serde_json::from_str(&failed)?,
                reported_at,
            })
        })
        .transpose()
    }
}

fn revision(conn: &rusqlite::Connection, owner: &str) -> Result<u64> {
    let revision: Option<i64> = conn
        .query_row(
            "SELECT revision FROM skill_revisions WHERE owner_id = ?1",
            [owner],
            |r| r.get(0),
        )
        .optional()?;
    Ok(revision.unwrap_or(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::Enrollment;
    use ed25519_dalek::SigningKey;
    use hennery_proto::frames::SkillFailure;
    use hennery_proto::skills::Frontmatter;

    const NOW: i64 = 1_800_000_000;

    fn inspected(name: &str, hash: &str) -> Inspected {
        Inspected {
            frontmatter: Frontmatter {
                name: name.into(),
                description: format!("{name} skill"),
            },
            skill_md: format!("---\nname: {name}\ndescription: {name} skill\n---\n"),
            content_hash: hash.into(),
        }
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

    fn create(hosts: &Hosts, name: &str) -> SkillRecord {
        hosts
            .create_skill(&inspected(name, &format!("hash-{name}")), name.as_bytes(), NOW)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn create_list_replace_delete_bump_the_revision() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        assert_eq!(hosts.skills_revision().unwrap(), 0);
        let b = create(&hosts, "b");
        assert_eq!(hosts.skills_revision().unwrap(), 1);
        let a = create(&hosts, "a");
        // The second create is a bump too; list is by name.
        assert_eq!(hosts.skills_revision().unwrap(), 2);
        let names: Vec<_> = hosts.list_skills().unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(hosts.skill(&a.id).unwrap().unwrap(), a);
        assert_eq!(hosts.skill_md(&a.id).unwrap().unwrap(), inspected("a", "").skill_md);
        assert_eq!(
            hosts.skill_archive(&a.id).unwrap().unwrap(),
            ("hash-a".into(), b"a".to_vec())
        );

        let replaced = hosts
            .replace_skill_archive(&b.id, &inspected("b", "hash-b2"), b"b2", NOW + 5)
            .unwrap()
            .unwrap();
        assert_eq!(
            (replaced.content_hash.as_str(), replaced.updated_at),
            ("hash-b2", NOW + 5)
        );
        assert_eq!(
            hosts.skill_archive(&b.id).unwrap().unwrap(),
            ("hash-b2".into(), b"b2".to_vec())
        );
        assert_eq!(hosts.skills_revision().unwrap(), 3);

        hosts.set_skill_hosts(&a.id, &["host-1".into()], NOW).unwrap().unwrap();
        assert_eq!(hosts.skills_revision().unwrap(), 4);
        assert_eq!(hosts.skill_hosts(&a.id).unwrap(), ["host-1"]);

        assert!(hosts.delete_skill(&a.id).unwrap());
        assert_eq!(hosts.skills_revision().unwrap(), 5);
        assert!(!hosts.delete_skill(&a.id).unwrap());
        assert_eq!(hosts.skills_revision().unwrap(), 5);
        assert!(hosts.skill(&a.id).unwrap().is_none());
    }

    #[test]
    fn a_duplicate_name_is_refused() {
        let hosts = Hosts::open_in_memory().unwrap();
        create(&hosts, "pdf");
        let again = hosts.create_skill(&inspected("pdf", "other"), b"x", NOW).unwrap();
        assert_eq!(again.unwrap_err(), SkillWrite::Exists);
        assert_eq!(hosts.skills_revision().unwrap(), 1);
    }

    #[test]
    fn the_owner_cap_is_256() {
        let hosts = Hosts::open_in_memory().unwrap();
        for i in 0..MAX_SKILLS {
            create(&hosts, &format!("s{i}"));
        }
        let over = hosts.create_skill(&inspected("one-more", "h"), b"x", NOW).unwrap();
        assert_eq!(over.unwrap_err(), SkillWrite::TooMany);
        assert_eq!(hosts.list_skills().unwrap().len(), 256);
    }

    #[test]
    fn replacing_with_another_name_is_refused() {
        let hosts = Hosts::open_in_memory().unwrap();
        let a = create(&hosts, "a");
        let got = hosts
            .replace_skill_archive(&a.id, &inspected("z", "h"), b"x", NOW)
            .unwrap();
        assert_eq!(got.unwrap_err(), SkillWrite::NameMismatch);
        let unknown = hosts
            .replace_skill_archive("nope", &inspected("a", "h"), b"x", NOW)
            .unwrap();
        assert_eq!(unknown.unwrap_err(), SkillWrite::Unknown);
        assert_eq!(hosts.skill(&a.id).unwrap().unwrap().content_hash, "hash-a");
    }

    #[test]
    fn sync_for_a_host_lists_only_its_assignments() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        host(&hosts, "host-2", 2);
        let a = create(&hosts, "a");
        create(&hosts, "b");
        assert_eq!(hosts.skill_sync_for_host("host-1").unwrap(), (2, vec![]));
        hosts.set_skill_hosts(&a.id, &["host-1".into()], NOW).unwrap().unwrap();
        let (revision, items) = hosts.skill_sync_for_host("host-1").unwrap();
        assert_eq!(revision, 3);
        assert_eq!(
            items,
            [SkillSyncItem {
                id: a.id.clone(),
                name: "a".into(),
                content_hash: "hash-a".into()
            }]
        );
        assert!(hosts.skill_sync_for_host("host-2").unwrap().1.is_empty());
    }

    #[test]
    fn an_unknown_or_revoked_host_cannot_be_assigned() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        host(&hosts, "host-2", 2);
        let a = create(&hosts, "a");
        hosts.revoke("host-2", NOW).unwrap();
        let before = hosts.skills_revision().unwrap();
        for bad in [vec!["ghost".to_string()], vec!["host-1".into(), "host-2".into()]] {
            assert_eq!(
                hosts.set_skill_hosts(&a.id, &bad, NOW).unwrap().unwrap_err(),
                SkillWrite::Unknown
            );
        }
        assert_eq!(
            hosts
                .set_skill_hosts("nope", &["host-1".into()], NOW)
                .unwrap()
                .unwrap_err(),
            SkillWrite::Unknown
        );
        assert!(hosts.skill_hosts(&a.id).unwrap().is_empty());
        assert_eq!(hosts.skills_revision().unwrap(), before);
    }

    /// A revoked host would otherwise lock the skill's host set: every later
    /// write naming it answers `Unknown` (final review, finding 2).
    #[test]
    fn revoking_a_host_drops_its_assignments_and_bumps_the_revision() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        host(&hosts, "host-2", 2);
        let a = create(&hosts, "a");
        hosts
            .set_skill_hosts(&a.id, &["host-1".into(), "host-2".into()], NOW)
            .unwrap()
            .unwrap();
        let before = hosts.skills_revision().unwrap();
        hosts.revoke("host-2", NOW).unwrap();
        assert_eq!(hosts.skill_hosts(&a.id).unwrap(), vec!["host-1".to_string()]);
        assert_eq!(hosts.skills_revision().unwrap(), before + 1);
        assert!(hosts.skill_sync_for_host("host-2").unwrap().1.is_empty());
        // The remaining set can be written again.
        hosts.set_skill_hosts(&a.id, &["host-1".into()], NOW).unwrap().unwrap();
        // A host with no assignments is revoked without a bump.
        let before = hosts.skills_revision().unwrap();
        host(&hosts, "host-3", 3);
        hosts.revoke("host-3", NOW).unwrap();
        assert_eq!(hosts.skills_revision().unwrap(), before);
    }

    #[test]
    fn host_state_is_recorded_and_read_back() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        assert!(hosts.skill_state("host-1").unwrap().is_none());
        let report = SkillHostReport {
            revision: 7,
            applied: vec!["s1".into()],
            failed: vec![SkillFailed {
                id: "s2".into(),
                reason: SkillFailure::Conflict,
                agent: Some("claude".into()),
            }],
            reported_at: NOW,
        };
        hosts.record_skill_state("host-1", &report).unwrap();
        assert_eq!(hosts.skill_state("host-1").unwrap().unwrap(), report);
        let later = SkillHostReport {
            revision: 8,
            applied: vec![],
            failed: vec![],
            reported_at: NOW + 1,
        };
        hosts.record_skill_state("host-1", &later).unwrap();
        assert_eq!(hosts.skill_state("host-1").unwrap().unwrap(), later);
    }

    #[test]
    fn deleting_a_skill_drops_its_assignments() {
        let hosts = Hosts::open_in_memory().unwrap();
        host(&hosts, "host-1", 1);
        let a = create(&hosts, "a");
        hosts.set_skill_hosts(&a.id, &["host-1".into()], NOW).unwrap().unwrap();
        assert!(hosts.delete_skill(&a.id).unwrap());
        let left: i64 = hosts
            .conn()
            .query_row("SELECT count(*) FROM skill_assignments", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0);
        assert!(hosts.skill_sync_for_host("host-1").unwrap().1.is_empty());
    }
}
