//! The host's side of skill delivery (skills spec §6): a `skill_sync`
//! is reconciled against the store, what is missing is fetched, and the
//! outcome goes back as one `skill_state` once nothing is left to fetch.

use crate::skills::{self, AgentDirs};
use crate::uplink::Uplink;
use base64::Engine;
use hennery_proto::frames::{HostFrame, SkillFailed, SkillFailure, SkillSyncItem};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The revision being applied, and what it still waits for.
#[derive(Default)]
struct Pending {
    /// The collector connection this belongs to: a frame read on an
    /// earlier connection is dropped (its thread may run late).
    generation: u64,
    revision: u64,
    /// The skills fetched but not yet delivered, by id.
    waiting: BTreeMap<String, SkillSyncItem>,
    applied: Vec<String>,
    failed: Vec<SkillFailed>,
}

/// The store, the environment the agents' directories are found in, and
/// the revision in progress.
pub struct SkillSync {
    store: skills::Store,
    agent_env: Vec<(String, String)>,
    pending: Mutex<Pending>,
}

impl SkillSync {
    pub fn new(store: skills::Store, agent_env: Vec<(String, String)>) -> Self {
        Self {
            store,
            agent_env,
            pending: Mutex::default(),
        }
    }

    /// The agents' skill directories, found again for each frame: an agent
    /// installed after the host started (a new `~/.codex`) is linked at
    /// the next sync, without a restart.
    fn dirs(&self) -> AgentDirs {
        skills::agent_dirs(&self.agent_env)
    }

    /// A new collector connection: its revisions are not comparable with
    /// the last one's (the collector may have been restored), and it
    /// resends `skill_sync` after every handshake.
    pub fn reset(&self) {
        let mut pending = self.lock();
        *pending = Pending {
            generation: pending.generation + 1,
            ..Pending::default()
        };
    }

    /// The current connection's generation, read where the frame is read,
    /// in order, and handed to `on_sync`/`on_archive` with it.
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `skill_sync`: reconcile, ask for what is missing, and answer at once
    /// when nothing is. A new revision replaces the one in progress; an
    /// older one is ignored within a connection (frames run on their own
    /// threads, so a sync can be applied after a later one), as is a frame
    /// of an earlier connection.
    pub fn on_sync(&self, uplink: &Uplink, generation: u64, revision: u64, items: Vec<SkillSyncItem>) {
        // Held throughout, so frames are applied one at a time.
        let mut pending = self.lock();
        if generation != pending.generation {
            tracing::debug!(revision, "a skill sync of an earlier connection");
            return;
        }
        if revision < pending.revision {
            tracing::debug!(revision, current = pending.revision, "an older skill sync");
            return;
        }
        let (applied, failed, fetch) = self.store.reconcile(&items, &self.dirs());
        for item in &fetch {
            uplink.reply(HostFrame::SkillFetch {
                id: item.id.clone(),
                content_hash: item.content_hash.clone(),
            });
        }
        *pending = Pending {
            generation,
            revision,
            waiting: fetch.into_iter().map(|item| (item.id.clone(), item)).collect(),
            applied,
            failed,
        };
        finish(uplink, &pending);
    }

    /// `skill_archive`: install what was fetched. An archive nobody waits
    /// for (an older revision, another hash) is dropped.
    pub fn on_archive(&self, uplink: &Uplink, generation: u64, id: &str, content_hash: &str, data: &str) {
        let mut pending = self.lock();
        if generation != pending.generation {
            tracing::debug!(id, "a skill archive of an earlier connection");
            return;
        }
        let Some(item) = pending
            .waiting
            .get(id)
            .filter(|item| item.content_hash == content_hash)
            .cloned()
        else {
            tracing::debug!(id, "a skill archive nobody waits for");
            return;
        };
        pending.waiting.remove(id);
        let failures = match base64::engine::general_purpose::STANDARD.decode(data) {
            Ok(tgz) => self.store.install(&item, &tgz, &self.dirs()),
            Err(_) => vec![SkillFailed {
                id: id.to_string(),
                reason: SkillFailure::Invalid,
                agent: None,
            }],
        };
        if failures.is_empty() {
            pending.applied.push(item.id);
        } else {
            pending.failed.extend(failures);
        }
        finish(uplink, &pending);
    }
}

/// Send `skill_state` when nothing is awaited.
fn finish(uplink: &Uplink, pending: &Pending) {
    if pending.waiting.is_empty() {
        uplink.reply(HostFrame::SkillState {
            revision: pending.revision,
            applied: pending.applied.clone(),
            failed: pending.failed.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::Outbox;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use tokio::sync::mpsc::UnboundedReceiver;

    fn tgz(name: &str) -> Vec<u8> {
        let skill_md = format!("---\nname: {name}\ndescription: does {name}\n---\n");
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(skill_md.len() as u64);
        builder
            .append_data(&mut header, "SKILL.md", skill_md.as_bytes())
            .unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&builder.into_inner().unwrap()).unwrap();
        gz.finish().unwrap()
    }

    fn item(id: &str, name: &str, archive: &[u8]) -> SkillSyncItem {
        SkillSyncItem {
            id: id.into(),
            name: name.into(),
            content_hash: hex::encode(Sha256::digest(archive)),
        }
    }

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    struct Env {
        tmp: tempfile::TempDir,
        sync: SkillSync,
        uplink: Uplink,
        replies: UnboundedReceiver<HostFrame>,
    }

    impl Env {
        /// A host whose home holds the agent roots `agents`.
        fn new(agents: &[&str]) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::create_dir(tmp.path().join("data")).unwrap();
            std::fs::create_dir(tmp.path().join("home")).unwrap();
            for agent in agents {
                std::fs::create_dir(tmp.path().join("home").join(agent)).unwrap();
            }
            let home = tmp.path().join("home").to_str().unwrap().to_string();
            let store = skills::Store::open(&tmp.path().join("data")).unwrap();
            let sync = SkillSync::new(store, vec![("HOME".into(), home)]);
            let (uplink, replies) = Uplink::new(Outbox::open_in_memory().unwrap());
            Self {
                tmp,
                sync,
                uplink,
                replies,
            }
        }

        fn link(&self, agent: &str, name: &str) -> std::path::PathBuf {
            self.tmp.path().join("home").join(agent).join("skills").join(name)
        }

        fn reply(&mut self) -> Option<HostFrame> {
            self.replies.try_recv().ok()
        }
    }

    fn state(revision: u64, applied: &[&str], failed: Vec<SkillFailed>) -> Option<HostFrame> {
        Some(HostFrame::SkillState {
            revision,
            applied: applied.iter().map(|s| s.to_string()).collect(),
            failed,
        })
    }

    /// Frames run on their own threads: a sync applied after a later one
    /// must not undo it (final review, finding 6).
    #[test]
    fn an_older_sync_after_a_newer_one_is_ignored() {
        let mut env = Env::new(&[".claude", ".codex"]);
        let archive = tgz("alpha");
        let alpha = item("s1", "alpha", &archive);
        let generation = env.sync.generation();
        env.sync.on_sync(&env.uplink, generation, 6, vec![alpha.clone()]);
        assert!(matches!(env.reply(), Some(HostFrame::SkillFetch { .. })));
        env.sync
            .on_archive(&env.uplink, generation, "s1", &alpha.content_hash, &b64(&archive));
        assert_eq!(env.reply(), state(6, &["s1"], vec![]));
        // Revision 5 would remove alpha: it is ignored, with no reply.
        env.sync.on_sync(&env.uplink, generation, 5, vec![]);
        assert_eq!(env.reply(), None);
        assert!(env.link(".claude", "alpha").join("SKILL.md").is_file());
        // The same revision again is answered, still for 6.
        env.sync.on_sync(&env.uplink, generation, 6, vec![alpha]);
        assert_eq!(env.reply(), state(6, &["s1"], vec![]));
    }

    /// An agent installed after the host started is linked at the next
    /// sync, without a restart (final review, finding 7).
    #[test]
    fn an_agent_dir_made_after_start_is_linked_at_the_next_sync() {
        let mut env = Env::new(&[".claude"]);
        let archive = tgz("alpha");
        let alpha = item("s1", "alpha", &archive);
        let generation = env.sync.generation();
        env.sync.on_sync(&env.uplink, generation, 1, vec![alpha.clone()]);
        assert!(matches!(env.reply(), Some(HostFrame::SkillFetch { .. })));
        env.sync
            .on_archive(&env.uplink, generation, "s1", &alpha.content_hash, &b64(&archive));
        let missing = SkillFailed {
            id: "s1".into(),
            reason: SkillFailure::AgentDirMissing,
            agent: Some("codex".into()),
        };
        assert_eq!(env.reply(), state(1, &[], vec![missing]));
        std::fs::create_dir(env.tmp.path().join("home/.codex")).unwrap();
        env.sync.on_sync(&env.uplink, generation, 2, vec![alpha]);
        assert_eq!(env.reply(), state(2, &["s1"], vec![]));
        assert!(env.link(".codex", "alpha").join("SKILL.md").is_file());
    }

    /// A thread left over from a previous connection cannot act after
    /// `reset`.
    #[test]
    fn a_frame_of_an_earlier_connection_is_ignored() {
        let mut env = Env::new(&[".claude", ".codex"]);
        let archive = tgz("alpha");
        let alpha = item("s1", "alpha", &archive);
        let old = env.sync.generation();
        env.sync.on_sync(&env.uplink, old, 3, vec![alpha.clone()]);
        assert!(matches!(env.reply(), Some(HostFrame::SkillFetch { .. })));
        env.sync.reset();
        let new = env.sync.generation();
        assert_ne!(old, new);
        env.sync.on_sync(&env.uplink, old, 9, vec![]);
        env.sync
            .on_archive(&env.uplink, old, "s1", &alpha.content_hash, &b64(&archive));
        assert_eq!(env.reply(), None);
        assert!(std::fs::symlink_metadata(env.link(".claude", "alpha")).is_err());
        // The new connection's own lower revision is applied.
        env.sync.on_sync(&env.uplink, new, 1, vec![]);
        assert_eq!(env.reply(), state(1, &[], vec![]));
    }
}
