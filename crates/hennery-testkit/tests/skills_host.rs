//! Skill delivery on a real host, played frame by frame by a fake collector
//! (skills spec §6): `skill_sync` in, `skill_fetch` and `skill_state` out.

use base64::Engine;
use futures::{SinkExt, StreamExt};
use hennery_host::HostConfig;
use hennery_host::identity::HostKey;
use hennery_proto::frames::{Capability, CollectorFrame, HostFrame, SkillFailed, SkillFailure, SkillSyncItem};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
}

/// The fake collector's end of one host connection.
struct Collector {
    ws: Ws,
}

impl Collector {
    /// Accept the host, answer its `hello`, and read up to its
    /// `resend_complete`: the capabilities it announced.
    async fn accept(listener: &TcpListener) -> (Self, Vec<Capability>) {
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let ws = tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await.unwrap();
        let mut collector = Self { ws };
        let HostFrame::Hello { capabilities, .. } = collector.next().await else {
            panic!("expected hello");
        };
        collector
            .send(&CollectorFrame::HelloAck {
                protocol_version: PROTOCOL_VERSION.into(),
                collector_version: "test".into(),
                committed: BTreeMap::new(),
            })
            .await;
        loop {
            if let HostFrame::ResendComplete = collector.next().await {
                break;
            }
        }
        (collector, capabilities.0)
    }

    async fn send(&mut self, frame: &CollectorFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> HostFrame {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("host connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a host frame within 20s")
    }

    /// The next `skill_fetch` or `skill_state`, whichever the host sends first.
    async fn skill_frame(&mut self) -> HostFrame {
        loop {
            let frame = self.next().await;
            if matches!(frame, HostFrame::SkillFetch { .. } | HostFrame::SkillState { .. }) {
                return frame;
            }
        }
    }

    async fn sync(&mut self, revision: u64, items: Vec<SkillSyncItem>) {
        self.send(&CollectorFrame::SkillSync { revision, items }).await;
    }

    async fn archive(&mut self, id: &str, content_hash: &str, data: String) {
        self.send(&CollectorFrame::SkillArchive {
            id: id.into(),
            content_hash: content_hash.into(),
            data,
        })
        .await;
    }
}

fn tgz(name: &str, body: &str) -> Vec<u8> {
    let skill_md = format!("---\nname: {name}\ndescription: does {name}\n---\n{body}\n");
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

struct Setup {
    _dir: tempfile::TempDir,
    base: PathBuf,
    listener: TcpListener,
}

struct Running {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(20), self.task)
            .await
            .expect("the host stops")
            .unwrap();
    }
}

impl Setup {
    async fn new() -> Self {
        // SAFETY: umask(2) cannot fail.
        unsafe { libc::umask(0o022) };
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        for sub in ["home", "home/.claude", "home/.codex", "host"] {
            std::fs::create_dir(base.join(sub)).unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self {
            _dir: dir,
            base,
            listener,
        }
    }

    fn link(&self, agent: &str, name: &str) -> PathBuf {
        self.base.join("home").join(agent).join("skills").join(name)
    }

    fn start_host(&self) -> Running {
        let mut cfg = HostConfig::new(
            format!("ws://{}/api/hosts/ws", self.listener.local_addr().unwrap()),
            "host-1",
            HostKey::from_seed([1; 32]),
            self.base.join("host"),
        );
        cfg.agent_env = vec![("HOME".into(), self.base.join("home").to_str().unwrap().into())];
        let (stop, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = hennery_host::connection::run_until(cfg, async {
                let _ = rx.await;
            })
            .await;
        });
        Running { stop, task }
    }
}

#[tokio::test]
async fn hello_announces_skills_and_a_sync_with_nothing_answers_state() {
    let setup = Setup::new().await;
    let _host = setup.start_host();
    let (mut collector, capabilities) = Collector::accept(&setup.listener).await;
    assert!(capabilities.contains(&Capability::Skills));
    collector.sync(1, vec![]).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 1,
            applied: vec![],
            failed: vec![]
        }
    );
}

#[tokio::test]
async fn a_sync_fetches_installs_and_reports() {
    let setup = Setup::new().await;
    let _host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    let archive = tgz("alpha", "one");
    let alpha = item("s1", "alpha", &archive);
    collector.sync(1, vec![alpha.clone()]).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillFetch {
            id: "s1".into(),
            content_hash: alpha.content_hash.clone()
        }
    );
    collector.archive("s1", &alpha.content_hash, b64(&archive)).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 1,
            applied: vec!["s1".into()],
            failed: vec![]
        }
    );
    for agent in [".claude", ".codex"] {
        let link = setup.link(agent, "alpha");
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert!(std::fs::read_to_string(link.join("SKILL.md")).unwrap().contains("one"));
    }
}

#[tokio::test]
async fn a_changed_skill_is_refetched_and_relinked() {
    let setup = Setup::new().await;
    let _host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    let first = tgz("alpha", "one");
    let first_item = item("s1", "alpha", &first);
    collector.sync(1, vec![first_item.clone()]).await;
    collector.skill_frame().await;
    collector.archive("s1", &first_item.content_hash, b64(&first)).await;
    collector.skill_frame().await;
    let before = std::fs::read_link(setup.link(".claude", "alpha")).unwrap();

    let second = tgz("alpha", "two");
    let second_item = item("s1", "alpha", &second);
    collector.sync(2, vec![second_item.clone()]).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillFetch {
            id: "s1".into(),
            content_hash: second_item.content_hash.clone()
        }
    );
    collector.archive("s1", &second_item.content_hash, b64(&second)).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 2,
            applied: vec!["s1".into()],
            failed: vec![]
        }
    );
    for agent in [".claude", ".codex"] {
        let link = setup.link(agent, "alpha");
        assert!(std::fs::read_to_string(link.join("SKILL.md")).unwrap().contains("two"));
    }
    assert_ne!(std::fs::read_link(setup.link(".claude", "alpha")).unwrap(), before);
}

#[tokio::test]
async fn a_bad_archive_is_reported_invalid_and_nothing_is_linked() {
    let setup = Setup::new().await;
    let _host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    let archive = tgz("alpha", "one");
    let alpha = item("s1", "alpha", &archive);
    collector.sync(1, vec![alpha.clone()]).await;
    collector.skill_frame().await;
    collector
        .archive("s1", &alpha.content_hash, "not base64 !!".into())
        .await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 1,
            applied: vec![],
            failed: vec![SkillFailed {
                id: "s1".into(),
                reason: SkillFailure::Invalid,
                agent: None
            }]
        }
    );
    for agent in [".claude", ".codex"] {
        assert!(std::fs::symlink_metadata(setup.link(agent, "alpha")).is_err());
    }
}

#[tokio::test]
async fn a_reconnect_resyncs_from_the_registry() {
    let setup = Setup::new().await;
    let host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    let archive = tgz("alpha", "one");
    let alpha = item("s1", "alpha", &archive);
    collector.sync(1, vec![alpha.clone()]).await;
    collector.skill_frame().await;
    collector.archive("s1", &alpha.content_hash, b64(&archive)).await;
    collector.skill_frame().await;
    drop(collector);
    host.stop().await;

    let _host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    collector.sync(2, vec![alpha]).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 2,
            applied: vec!["s1".into()],
            failed: vec![]
        }
    );
}

#[tokio::test]
async fn a_new_connection_accepts_a_lower_revision() {
    let setup = Setup::new().await;
    let _host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    collector.sync(5, vec![]).await;
    assert!(matches!(
        collector.skill_frame().await,
        HostFrame::SkillState { revision: 5, .. }
    ));
    drop(collector);
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    collector.sync(1, vec![]).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 1,
            applied: vec![],
            failed: vec![]
        }
    );
}

/// A gzip tarball as `tar -czf x.tgz -C dir .` writes it: a `./` root entry
/// and every name under `./`. Names are written raw into the header.
fn tar_c_dot(name: &str) -> Vec<u8> {
    let skill_md = format!("---\nname: {name}\ndescription: does {name}\n---\nbody\n");
    let entries: [(&str, tar::EntryType, u32, &[u8]); 4] = [
        ("./", tar::EntryType::Directory, 0o755, b""),
        ("./SKILL.md", tar::EntryType::Regular, 0o644, skill_md.as_bytes()),
        ("./scripts/", tar::EntryType::Directory, 0o755, b""),
        ("./scripts/run.sh", tar::EntryType::Regular, 0o755, b"#!/bin/sh\n"),
    ];
    let mut builder = tar::Builder::new(Vec::new());
    for (path, kind, mode, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_size(body.len() as u64);
        header.set_cksum();
        builder.append(&header, body).unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

/// The collector and the host accept the same archives (final review,
/// finding 1): a `tar -C dir .` archive the collector's inspector accepts
/// is installed and linked by a real host.
#[tokio::test]
async fn an_archive_the_collector_accepts_is_installed_by_the_host() {
    let archive = tar_c_dot("alpha");
    let inspected = hennery_kernel::skill_archive::inspect(&archive).expect("the collector accepts it");
    assert_eq!(inspected.frontmatter.name, "alpha");

    let setup = Setup::new().await;
    let _host = setup.start_host();
    let (mut collector, _) = Collector::accept(&setup.listener).await;
    let alpha = item("s1", "alpha", &archive);
    assert_eq!(alpha.content_hash, inspected.content_hash);
    collector.sync(1, vec![alpha.clone()]).await;
    collector.skill_frame().await;
    collector.archive("s1", &alpha.content_hash, b64(&archive)).await;
    assert_eq!(
        collector.skill_frame().await,
        HostFrame::SkillState {
            revision: 1,
            applied: vec!["s1".into()],
            failed: vec![]
        }
    );
    for agent in [".claude", ".codex"] {
        let link = setup.link(agent, "alpha");
        assert!(link.join("SKILL.md").is_file());
        assert!(link.join("scripts/run.sh").is_file());
    }
}
