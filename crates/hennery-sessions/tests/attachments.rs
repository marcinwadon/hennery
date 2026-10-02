//! A prompt's images in the sessions store (ACP core §7, §8; plan 6a):
//! stored once each as files beside the database, referenced from the turn
//! and its `user_turn` event, never carried in either.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hennery_proto::frames::SessionBody;
use hennery_proto::rest::{AttachmentUsage, EventDto};
use hennery_sessions::content::{self, Checked};
use hennery_sessions::store::Store;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// `len` bytes of a PNG, different for each `seed`.
fn png(seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..len - 8).map(|i| seed.wrapping_add(i as u8)));
    bytes
}

fn image(bytes: &[u8]) -> Value {
    json!({ "type": "image", "mimeType": "image/png", "data": STANDARD.encode(bytes) })
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn started(store: &Store) {
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
        .unwrap();
}

/// Check `content`, save its images and open turn `turn` with it, as the
/// prompt route does.
fn prompt(store: &Store, turn: &str, content: Vec<Value>) -> Checked {
    let checked = content::check(content).unwrap();
    store.save_images(&checked.images).unwrap();
    assert!(store.open_prompt("s1", turn, &checked).unwrap());
    checked
}

fn turn_started(store: &Store, seq: u64, turn: &str) -> Vec<EventDto> {
    let body = SessionBody::TurnStarted {
        request_id: format!("r{seq}"),
        turn_id: turn.into(),
    };
    store.ingest("s1", seq, &body).unwrap()
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_prompts_images_are_stored_once_each_and_referenced_never_carried() {
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    started(&store);
    let (a, b) = (png(1, 4096), png(2, 100));
    let checked = prompt(
        &store,
        "t1",
        vec![
            json!({ "type": "text", "text": "these" }),
            image(&a),
            image(&a),
            image(&b),
        ],
    );

    // One file per image, named by its hash, private, in `attachments/`.
    let dir = data.path().join("attachments");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut expected = vec![sha(&a), sha(&b)];
    expected.sort();
    assert_eq!(names, expected);
    assert_eq!(std::fs::read(dir.join(sha(&a))).unwrap(), a);
    assert_eq!((mode(&dir), mode(&dir.join(sha(&b)))), (0o700, 0o600));

    // The turn holds the references, not the bytes.
    let conn = rusqlite::Connection::open(&db).unwrap();
    let stored: String = conn
        .query_row("SELECT content FROM turns WHERE turn_id = 't1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap(),
        json!(checked.stored_json())
    );
    assert!(!stored.contains("\"data\""), "{stored}");

    // So does the `user_turn` event, and each image block is linked to it
    // by its position in the content.
    let created = turn_started(&store, 2, "t1");
    let user_turn = created.iter().find(|e| e.kind == "user_turn").unwrap();
    assert_eq!(user_turn.body["content"], json!(checked.stored_json()));
    assert_eq!(user_turn.body["content"][1]["sha256"], sha(&a));
    let links: Vec<(i64, String, i64)> = conn
        .prepare("SELECT event_id, sha256, position FROM event_attachments ORDER BY position")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let id = user_turn.event_id;
    assert_eq!(links, [(id, sha(&a), 1), (id, sha(&a), 2), (id, sha(&b), 3)]);

    // A replay of the timeline carries no image's bytes.
    let replay = serde_json::to_string(&store.events("s1", 0, 100).unwrap()).unwrap();
    assert!(!replay.contains(&STANDARD.encode(&b)), "the replay carries an image");

    // Each image counts once, however often it was sent.
    let usage = store.attachment_usage().unwrap();
    assert_eq!(
        usage,
        AttachmentUsage {
            count: 2,
            bytes: (a.len() + b.len()) as u64
        }
    );
}

#[test]
fn the_same_image_in_a_later_prompt_is_the_same_file_and_row() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(&data.path().join("hennery.db")).unwrap();
    started(&store);
    let a = png(1, 64);
    prompt(&store, "t1", vec![image(&a)]);
    turn_started(&store, 2, "t1");
    store
        .ingest(
            "s1",
            3,
            &SessionBody::TurnEnded {
                turn_id: "t1".into(),
                outcome: hennery_proto::frames::TurnOutcome::Completed,
                stop_reason: None,
                error: None,
            },
        )
        .unwrap();
    prompt(&store, "t2", vec![image(&a)]);
    turn_started(&store, 4, "t2");
    assert_eq!(store.attachment_usage().unwrap().count, 1);
    assert_eq!(std::fs::read_dir(data.path().join("attachments")).unwrap().count(), 1);
}

#[test]
fn an_attachment_is_read_back_with_its_type_and_only_by_its_hash() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(&data.path().join("hennery.db")).unwrap();
    started(&store);
    let a = png(1, 64);
    prompt(&store, "t1", vec![image(&a)]);
    let found = store.attachment(&sha(&a)).unwrap().unwrap();
    assert_eq!((found.mime.as_str(), found.bytes), ("image/png", a));
    assert!(store.attachment(&"0".repeat(64)).unwrap().is_none());
    // Names that are not a hash are not looked up at all.
    for name in ["../hennery.db", "hennery.db", &sha(b"x").to_uppercase()] {
        assert!(store.attachment(name).unwrap().is_none(), "{name}");
    }
}

#[test]
fn a_turn_that_does_not_open_records_no_attachment() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(&data.path().join("hennery.db")).unwrap();
    started(&store);
    prompt(&store, "t1", vec![json!({ "type": "text", "text": "first" })]);
    let second = content::check(vec![image(&png(1, 64))]).unwrap();
    assert!(!store.open_prompt("s1", "t2", &second).unwrap());
    assert_eq!(
        store.attachment_usage().unwrap(),
        AttachmentUsage { count: 0, bytes: 0 }
    );
}

#[test]
fn an_in_memory_store_keeps_no_attachment_files() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let checked = content::check(vec![image(&png(1, 64))]).unwrap();
    assert!(store.save_images(&checked.images).is_err());
    // Text alone needs no file.
    prompt(&store, "t1", vec![json!({ "type": "text", "text": "hi" })]);
}

/// A turn stored before plan 6a may hold an image's bytes (decision 9):
/// it is left as it is, and links nothing.
#[test]
fn content_stored_before_attachments_links_nothing() {
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    started(&store);
    let old = json!({ "type": "image", "mimeType": "image/png", "data": STANDARD.encode(png(1, 64)) });
    assert!(store.open_turn("s1", "t1", std::slice::from_ref(&old)).unwrap());
    let created = turn_started(&store, 2, "t1");
    assert_eq!(created.last().unwrap().body["content"], json!([old]));
    let conn = rusqlite::Connection::open(&db).unwrap();
    let links: i64 = conn
        .query_row("SELECT count(*) FROM event_attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(links, 0);
}

/// A stored block naming an image of no row of the owner's (written by
/// hand, or by a client before 6a) links nothing, and the `user_turn` is
/// still written: without the link's guard, the foreign key would fail the
/// whole ingest of `turn_started`.
#[test]
fn a_block_naming_an_unknown_attachment_links_nothing() {
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("hennery.db");
    let store = Store::open(&db).unwrap();
    started(&store);
    let stray = json!({ "type": "image", "mimeType": "image/png", "sha256": "0".repeat(64), "size": 1 });
    assert!(store.open_turn("s1", "t1", std::slice::from_ref(&stray)).unwrap());
    let created = turn_started(&store, 2, "t1");
    assert_eq!(created.last().unwrap().kind, "user_turn");
    let conn = rusqlite::Connection::open(&db).unwrap();
    let links: i64 = conn
        .query_row("SELECT count(*) FROM event_attachments", [], |r| r.get(0))
        .unwrap();
    assert_eq!(links, 0);
}

/// An image whose file is gone is not found, rather than served empty.
#[test]
fn a_recorded_image_whose_file_is_gone_is_not_found() {
    let data = tempfile::tempdir().unwrap();
    let store = Store::open(&data.path().join("hennery.db")).unwrap();
    started(&store);
    let a = png(1, 64);
    prompt(&store, "t1", vec![image(&a)]);
    std::fs::remove_file(data.path().join("attachments").join(sha(&a))).unwrap();
    assert!(store.attachment(&sha(&a)).unwrap().is_none());
}
