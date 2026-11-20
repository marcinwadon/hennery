//! Pushing skills to hosts and answering their fetches and reports (skills
//! spec §6). The manifest is a full snapshot, so a resend is always safe.
use crate::hub::Hub;
use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::skills::SkillHostReport;
use hennery_proto::frames::{Capability, CollectorFrame, SkillFailed};

/// Send `host_id` its current manifest. A host without `Capability::Skills`
/// gets nothing (the hub's gate refuses it too).
pub(crate) fn resend_skills(hosts: &Hosts, hub: &Hub, host_id: &str) -> Result<()> {
    if !hub.has_capability(host_id, Capability::Skills) {
        return Ok(());
    }
    let (revision, items) = hosts.skill_sync_for_host(host_id)?;
    hub.notify(host_id, CollectorFrame::SkillSync { revision, items });
    Ok(())
}

/// `resend_skills` for every connected host; after every skills write.
pub(crate) fn broadcast_skills(hosts: &Hosts, hub: &Hub) -> Result<()> {
    for host in hub.connected_hosts() {
        resend_skills(hosts, hub, &host)?;
    }
    Ok(())
}

/// Answer a fetch with the archive when the skill is assigned to `host_id`
/// and `content_hash` is still current. A stale or unknown hash gets no
/// answer: the host receives a fresh manifest on the next change. A skill
/// not assigned to the host is never sent to it.
pub(crate) fn on_fetch(hosts: &Hosts, hub: &Hub, host_id: &str, id: &str, content_hash: &str) -> Result<()> {
    if !hosts.skill_hosts(id)?.iter().any(|assigned| assigned == host_id) {
        tracing::info!(%host_id, %id, "ignoring a fetch of a skill not assigned to the host");
        return Ok(());
    }
    match hosts.skill_archive(id)? {
        Some((hash, bytes)) if hash == content_hash => {
            hub.notify(
                host_id,
                CollectorFrame::SkillArchive {
                    id: id.to_string(),
                    content_hash: hash,
                    data: STANDARD.encode(bytes),
                },
            );
        }
        _ => tracing::info!(%host_id, %id, %content_hash, "ignoring a fetch of a stale or unknown skill"),
    }
    Ok(())
}

/// Keep what the host reported.
pub(crate) fn on_state(
    hosts: &Hosts,
    host_id: &str,
    revision: u64,
    applied: Vec<String>,
    failed: Vec<SkillFailed>,
    now: i64,
) -> Result<()> {
    hosts.record_skill_state(
        host_id,
        &SkillHostReport {
            revision,
            applied,
            failed,
            reported_at: now,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hennery_kernel::hosts::Enrollment;
    use hennery_kernel::skill_archive::Inspected;
    use hennery_proto::frames::AgentIsolation;
    use hennery_proto::frames::{Capabilities, SkillFailure};
    use tokio::sync::mpsc;

    fn hosts_with_skill() -> (Hosts, String) {
        let hosts = Hosts::open_in_memory().unwrap();
        hosts
            .register(
                "h",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "laptop".into(),
                    host_version: "0".into(),
                    platform: "linux".into(),
                },
                1,
            )
            .unwrap();
        let skill = hosts
            .create_skill(
                &Inspected {
                    frontmatter: hennery_proto::skills::Frontmatter {
                        name: "pdf".into(),
                        description: "d".into(),
                    },
                    skill_md: "---\nname: pdf\ndescription: d\n---\n".into(),
                    content_hash: "hash-1".into(),
                },
                b"archive-bytes",
                1,
            )
            .unwrap()
            .unwrap();
        hosts.set_skill_hosts(&skill.id, &["h".into()], 1).unwrap().unwrap();
        (hosts, skill.id)
    }

    fn connect(hub: &Hub, caps: Vec<Capability>) -> mpsc::UnboundedReceiver<CollectorFrame> {
        let (tx, rx) = mpsc::unbounded_channel();
        let registration = hub
            .register("h", tx, Capabilities(caps), AgentIsolation::default())
            .unwrap();
        hub.mark_ready("h", registration.conn_id);
        rx
    }

    #[test]
    fn an_old_host_takes_no_skill_sync() {
        let (hosts, _) = hosts_with_skill();
        let old = Hub::new();
        let mut old_rx = connect(&old, vec![]);
        assert!(!old.notify(
            "h",
            CollectorFrame::SkillSync {
                revision: 1,
                items: vec![]
            }
        ));
        resend_skills(&hosts, &old, "h").unwrap();
        broadcast_skills(&hosts, &old).unwrap();
        assert!(old_rx.try_recv().is_err());

        let new = Hub::new();
        let mut new_rx = connect(&new, vec![Capability::Skills]);
        resend_skills(&hosts, &new, "h").unwrap();
        let Ok(CollectorFrame::SkillSync { revision, items }) = new_rx.try_recv() else {
            panic!("no skill_sync");
        };
        assert_eq!((revision, items.len()), (2, 1));
    }

    #[test]
    fn a_fetch_with_the_current_hash_is_answered_with_the_archive() {
        let (hosts, id) = hosts_with_skill();
        let hub = Hub::new();
        let mut rx = connect(&hub, vec![Capability::Skills]);
        on_fetch(&hosts, &hub, "h", &id, "hash-1").unwrap();
        let Ok(CollectorFrame::SkillArchive {
            id: got,
            content_hash,
            data,
        }) = rx.try_recv()
        else {
            panic!("no skill_archive");
        };
        assert_eq!((got, content_hash), (id, "hash-1".to_string()));
        assert_eq!(STANDARD.decode(data).unwrap(), b"archive-bytes");
    }

    #[test]
    fn a_fetch_with_a_stale_hash_is_ignored() {
        let (hosts, id) = hosts_with_skill();
        let hub = Hub::new();
        let mut rx = connect(&hub, vec![Capability::Skills]);
        on_fetch(&hosts, &hub, "h", &id, "old-hash").unwrap();
        on_fetch(&hosts, &hub, "h", "unknown", "hash-1").unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_fetch_from_an_unassigned_host_gets_no_archive() {
        let (hosts, id) = hosts_with_skill();
        hosts.set_skill_hosts(&id, &[], 1).unwrap().unwrap();
        let hub = Hub::new();
        let mut rx = connect(&hub, vec![Capability::Skills]);
        on_fetch(&hosts, &hub, "h", &id, "hash-1").unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_state_report_is_stored() {
        let (hosts, id) = hosts_with_skill();
        let failed = vec![SkillFailed {
            id: "x".into(),
            reason: SkillFailure::Io,
            agent: None,
        }];
        on_state(&hosts, "h", 2, vec![id.clone()], failed.clone(), 42).unwrap();
        let report = hosts.skill_state("h").unwrap().unwrap();
        assert_eq!(
            (report.revision, report.applied, report.failed, report.reported_at),
            (2, vec![id], failed, 42)
        );
    }
}
