use base64::{Engine, engine::general_purpose::STANDARD};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::{files::PublishArtifact, frames::SessionBody, rest::StoredBlock};
use hennery_sessions::{
    content,
    store::{Deletion, Store, Unattached},
};
use serde_json::json;

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    hat: String,
}
impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let hosts = Hosts::open(&db).unwrap();
        hosts
            .register(
                "host",
                &Enrollment {
                    public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
                    name: "test".into(),
                    host_version: "0".into(),
                    platform: "linux".into(),
                },
                1,
            )
            .unwrap();
        let hat = hosts.host("host").unwrap().unwrap().default_hat_id;
        let store = Store::open(&db).unwrap();
        for id in ["session", "other"] {
            store.create_session(id, "host", "fake", "/tmp", &hat, None).unwrap();
            store
                .ingest(id, 1, &SessionBody::session_started("start", format!("agent-{id}")))
                .unwrap();
        }
        Self { _dir: dir, store, hat }
    }
    fn publish(&self, q: &PublishArtifact, bytes: &[u8]) -> hennery_proto::files::ArtifactRevision {
        self.store
            .publish_artifact("session", "host", &self.hat, q, "report.md", bytes, None)
            .unwrap()
            .0
    }
}
fn request(key: &str) -> PublishArtifact {
    PublishArtifact {
        path: "report.md".into(),
        request_key: key.into(),
        title: None,
        mime_type: None,
        artifact_id: None,
        expected_revision: None,
    }
}

#[test]
fn publication_updates_are_immutable_and_retry_safe() {
    let w = World::new();
    let mut first = request("first");
    first.title = Some("Named analysis".into());
    let a = w.publish(&first, b"# First");
    let retry = w
        .store
        .publish_artifact(
            "session",
            "host",
            &w.hat,
            &first,
            "report.md",
            b"changed since publication",
            None,
        )
        .unwrap();
    assert_eq!(retry.0, a);
    assert!(retry.1.is_none());
    assert_eq!(w.store.artifact_retry("session", &first).unwrap(), Some(a.clone()));
    let mut update = request("second");
    update.artifact_id = Some(a.artifact_id.clone());
    update.expected_revision = Some(1);
    let b = w.publish(&update, b"# Second");
    assert_eq!(b.title, "Named analysis");
    assert_eq!(a.artifact_id, b.artifact_id);
    assert_eq!(b.revision, 2);
    assert_eq!(w.store.attachment(&a.sha256).unwrap().unwrap().bytes, b"# First");
    assert_eq!(w.store.attachment(&b.sha256).unwrap().unwrap().bytes, b"# Second");
    assert_eq!(
        w.store
            .artifact_revisions("session", Some(&a.artifact_id), u32::MAX)
            .unwrap(),
        vec![b, a]
    );
    assert!(w.store.artifact_revisions("other", None, u32::MAX).unwrap().is_empty());
}

#[test]
fn stale_and_cross_session_updates_do_not_overwrite() {
    let w = World::new();
    let a = w.publish(&request("a"), b"a");
    let mut q = request("b");
    q.artifact_id = Some(a.artifact_id);
    q.expected_revision = Some(1);
    w.publish(&q, b"b");
    q.request_key = "stale".into();
    assert_eq!(
        w.store
            .publish_artifact("session", "host", &w.hat, &q, "report.md", b"stale", None)
            .unwrap_err()
            .to_string(),
        "artifact_revision_conflict"
    );
    assert_eq!(
        w.store
            .publish_artifact("other", "host", &w.hat, &q, "report.md", b"escape", None)
            .unwrap_err()
            .to_string(),
        "artifact_revision_conflict"
    );
    let mut changed = request("a");
    changed.path = "other.md".into();
    assert_eq!(
        w.store.artifact_retry("session", &changed).unwrap_err().to_string(),
        "request_conflict"
    );
}

#[test]
fn deletion_and_sweep_preserve_only_referenced_blobs() {
    let w = World::new();
    let a = w.publish(&request("a"), b"shared");
    let b = w
        .store
        .publish_artifact("other", "host", &w.hat, &request("b"), "other.txt", b"shared", None)
        .unwrap()
        .0;
    assert_eq!(a.sha256, b.sha256);
    w.store
        .sweep_attachments(
            std::time::SystemTime::now() + std::time::Duration::from_secs(7200),
            &tokio_util::sync::CancellationToken::new(),
        )
        .unwrap();
    assert!(w.store.attachment(&a.sha256).unwrap().is_some());
    for id in ["session", "other"] {
        assert!(matches!(
            w.store
                .delete_session(
                    id,
                    Some(&Unattached {
                        lifecycle: "active".into(),
                        presumed_parked: false
                    })
                )
                .unwrap(),
            Deletion::Done { .. }
        ));
        assert!(w.store.artifact_revisions(id, None, u32::MAX).is_err());
        assert_eq!(w.store.attachment(&a.sha256).unwrap().is_some(), id == "session");
    }
    assert!(
        w.store
            .publish_artifact("session", "host", &w.hat, &request("after"), "x", b"x", None)
            .is_err()
    );
}

#[test]
fn generic_inputs_keep_name_and_bytes_across_storage_and_queue_edits() {
    let w = World::new();
    let bytes = b"name,value\na,42\n";
    let checked = content::check(vec![
        json!({"type":"text","text":"Analyze"}),
        json!({"type":"file","name":"data.csv","mimeType":"text/csv","data":STANDARD.encode(bytes),"uri":"ignored"}),
    ])
    .unwrap();
    assert!(matches!(&checked.stored[1],StoredBlock::File{name,..} if name=="data.csv"));
    assert!(checked.sent[1].get("uri").is_none());
    let id = w.store.enqueue_operator_prompt("session", &checked).unwrap();
    w.store.edit_operator_prompt("session", &id, "Analyze updated").unwrap();
    let files = w.store.attachment_usage().unwrap();
    assert_eq!(files.count, 1);
    let file = &checked.images[0];
    assert_eq!(w.store.attachment(&file.sha256).unwrap().unwrap().bytes, bytes);
}

#[test]
fn file_metadata_cannot_become_a_path_or_response_header() {
    for name in ["../secret", "a/b", "a\\b", "..", "a\r\nX: yes", ""] {
        assert!(
            content::check(vec![
                json!({"type":"file","name":name,"mimeType":"text/plain","data":"eA=="})
            ])
            .is_err(),
            "{name:?}"
        );
    }
    assert!(
        content::check(vec![
            json!({"type":"file","name":"ok.txt","mimeType":"text/html\r\nX:yes","data":"eA=="})
        ])
        .is_err()
    );
}

#[test]
fn artifact_pages_keep_latest_revisions_and_advance_without_duplicates() {
    let w = World::new();
    for n in 0..52 {
        w.publish(&request(&format!("publication-{n}")), b"same bytes");
    }
    let first = w.store.artifact_page("session", None).unwrap();
    assert_eq!(first.artifacts.len(), 50);
    let second = w.store.artifact_page("session", first.next_cursor.as_deref()).unwrap();
    assert_eq!(second.artifacts.len(), 2);
    assert!(second.next_cursor.is_none());
    assert!(
        !first
            .artifacts
            .iter()
            .any(|a| second.artifacts.iter().any(|b| a.artifact_id == b.artifact_id))
    );
    let a = &first.artifacts[0];
    let mut q = request("update");
    q.artifact_id = Some(a.artifact_id.clone());
    q.expected_revision = Some(1);
    w.publish(&q, b"new version");
    let page = w.store.artifact_page("session", None).unwrap();
    assert_eq!(page.artifacts[0].revision, 2);
    assert_eq!(page.artifacts.len(), 50);
    assert!(w.store.artifact_page("missing", None).is_err());
    assert!(w.store.artifact_page("session", Some("invalid")).is_err());
}

#[test]
fn deleting_a_generic_input_leaves_a_durable_host_cleanup_request() {
    let w = World::new();
    let prompt = content::check(vec![
        json!({"type":"file","name":"input.txt","mimeType":"text/plain","data":"eA=="}),
    ])
    .unwrap();
    w.store.save_images(&prompt.images).unwrap();
    assert!(w.store.open_prompt("session", "turn", &prompt).unwrap());
    assert_eq!(w.store.session_file_inputs("session").unwrap(), prompt.stored);
    assert!(w.store.file_cleanup_pending("host").unwrap().is_empty());
    w.store
        .delete_session(
            "session",
            Some(&Unattached {
                lifecycle: "active".into(),
                presumed_parked: false,
            }),
        )
        .unwrap();
    assert_eq!(w.store.file_cleanup_pending("host").unwrap(), vec!["session"]);
    w.store.file_cleanup_done("unrelated", "session").unwrap();
    assert_eq!(w.store.file_cleanup_pending("host").unwrap(), vec!["session"]);
    w.store.file_cleanup_done("host", "session").unwrap();
    assert!(w.store.file_cleanup_pending("host").unwrap().is_empty());
}
