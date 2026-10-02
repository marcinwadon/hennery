//! The orphan sweep (plan 9b decision 9, A14): the owner's
//! `attachments` rows nothing of theirs shows, image files no row of any
//! owner names, and leftover `.tmp` files, each file only past the grace.
//!
//! The file tests sweep at a `now` two hours ahead, so everything they
//! create is past the grace unless its mtime is set to that `now`: what
//! keeps a file is then the check under test, not its age.

use base64::Engine;
use hennery_proto::frames::SessionBody;
use hennery_sessions::content;
use hennery_sessions::store::{Store, SweepReport};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const HOUR: Duration = Duration::from_secs(3600);
const OTHER_OWNER: &str = "owner-00000000000000b2";

fn file_store(dir: &Path) -> (Store, PathBuf) {
    let db = dir.join("hennery.db");
    (Store::open(&db).unwrap(), db)
}

fn files(db: &Path) -> PathBuf {
    db.parent().unwrap().join(hennery_sessions::attachments::DIR)
}

/// `len` bytes of a PNG, different for each `seed`.
fn png(seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..len - 8).map(|i| seed.wrapping_add(i as u8)));
    bytes
}

fn image(bytes: &[u8]) -> Value {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    json!({ "type": "image", "mimeType": "image/png", "data": data })
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn active(store: &Store, id: &str) {
    store.create_session(id, "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest(id, 1, &SessionBody::session_started("r0", "a0")).unwrap();
}

/// Save `content`'s images and open `turn` of `session` with it, as the
/// prompt route does.
fn prompt_in(store: &Store, session: &str, turn: &str, content: Vec<Value>) {
    let checked = content::check(content).unwrap();
    store.save_images(&checked.images).unwrap();
    assert!(store.open_prompt(session, turn, &checked).unwrap());
}

fn set_mtime(path: &Path, at: SystemTime) {
    std::fs::File::open(path).unwrap().set_modified(at).unwrap();
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::symlink_metadata(path).unwrap().modified().unwrap()
}

/// A file of `name` in `dir`, holding `bytes`.
fn plant(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn other_owner_row(db: &Path, sha256: &str) {
    let conn = Connection::open(db).unwrap();
    conn.execute(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES (?1, 9223372036854775807, 9223372036854775807)
         ON CONFLICT DO NOTHING",
        [OTHER_OWNER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES (?1, ?2, 'image/png', 1, 't')",
        [OTHER_OWNER, sha256],
    )
    .unwrap();
}

fn rows(db: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare("SELECT owner_id, sha256 FROM attachments ORDER BY owner_id, sha256")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A temporary file's name as `attachments::write` makes it.
fn temp_name(sha256: &str, random: &str) -> String {
    format!(".{sha256}.{random}.tmp")
}

/// Decision 9: a row of the owner's that no turn and no event of theirs
/// shows goes; one a turn alone shows, or an event alone, stays, and so
/// does another owner's, unreferenced as it is.
#[test]
fn the_sweep_deletes_the_owners_rows_nothing_of_theirs_shows() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let (by_event, by_turn, unshown, foreign) = (png(1, 100), png(2, 100), png(3, 100), png(4, 100));
    active(&store, "s1");
    active(&store, "s2");
    // s1's image is shown by its `user_turn` event alone, as for a turn of
    // a database from before the turn links were kept.
    prompt_in(&store, "s1", "t1", vec![image(&by_event)]);
    store
        .ingest(
            "s1",
            2,
            &SessionBody::TurnStarted {
                request_id: "r1".into(),
                turn_id: "t1".into(),
            },
        )
        .unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute("DELETE FROM turn_attachments WHERE turn_id = 't1'", [])
        .unwrap();
    // s2's turn never started: no event, only the turn shows its image.
    prompt_in(&store, "s2", "t2", vec![image(&by_turn)]);
    conn.execute(
        "INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES (?1, ?2, 'image/png', 100, 't')",
        [store.owner_id(), &sha(&unshown)],
    )
    .unwrap();
    other_owner_row(&db, &sha(&foreign));

    let report = store.sweep_attachments(SystemTime::now()).unwrap();
    assert_eq!(report.rows, 1, "{report:?}");
    let owner = store.owner_id().to_string();
    let mut kept = vec![
        (owner.clone(), sha(&by_event)),
        (owner, sha(&by_turn)),
        (OTHER_OWNER.to_string(), sha(&foreign)),
    ];
    kept.sort();
    assert_eq!(rows(&db), kept);
    assert!(store.attachment(&sha(&by_event)).unwrap().is_some());
    assert!(store.attachment(&sha(&by_turn)).unwrap().is_some());
}

/// Decision 9: a file no row names goes once it is past the grace; a
/// younger one stays, and so does an old one a row of the owner's names.
/// A row the sweep drops takes its file in the same sweep.
#[test]
fn an_orphan_file_goes_past_the_grace_and_a_young_or_named_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let now = SystemTime::now() + 2 * HOUR;
    let (named, orphan, young, dropped) = (png(1, 100), png(2, 100), png(3, 100), png(4, 100));
    active(&store, "s1");
    prompt_in(&store, "s1", "t1", vec![image(&named)]);
    let dir = files(&db);
    let orphan_path = plant(&dir, &sha(&orphan), &orphan);
    let young_path = plant(&dir, &sha(&young), &young);
    set_mtime(&young_path, now - HOUR + Duration::from_secs(60));
    // A row nothing shows, and its file: the row goes first, then the file.
    let dropped_path = plant(&dir, &sha(&dropped), &dropped);
    Connection::open(&db)
        .unwrap()
        .execute(
            "INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES (?1, ?2, 'image/png', 100, 't')",
            [store.owner_id(), &sha(&dropped)],
        )
        .unwrap();

    let report = store.sweep_attachments(now).unwrap();
    assert_eq!(
        report,
        SweepReport {
            rows: 1,
            files: 2,
            temps: 0
        }
    );
    assert!(!orphan_path.exists() && !dropped_path.exists());
    assert!(young_path.exists());
    assert!(dir.join(sha(&named)).exists());
    assert!(store.attachment(&sha(&named)).unwrap().is_some());
}

/// A6: files are shared by hash, so a file another owner's row names stays,
/// though no row of this owner's names it.
#[test]
fn a_file_another_owners_row_names_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let theirs = png(5, 100);
    let path = plant(&files(&db), &sha(&theirs), &theirs);
    other_owner_row(&db, &sha(&theirs));

    let report = store.sweep_attachments(SystemTime::now() + 2 * HOUR).unwrap();
    assert_eq!(report, SweepReport::default());
    assert!(path.exists(), "another owner's image lost its file");
}

/// Decision 9 (plan 6a decision 7): a `.tmp` file a crashed write left goes
/// past the grace; a fresh one, perhaps a write still running, stays.
#[test]
fn a_stale_temporary_file_goes_and_a_fresh_one_stays() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let now = SystemTime::now() + 2 * HOUR;
    let bytes = png(6, 100);
    let dir = files(&db);
    let stale = plant(&dir, &temp_name(&sha(&bytes), "0123456789abcdef"), &bytes);
    let fresh = plant(&dir, &temp_name(&sha(&bytes), "fedcba9876543210"), &bytes);
    set_mtime(&fresh, now - HOUR + Duration::from_secs(60));

    let report = store.sweep_attachments(now).unwrap();
    assert_eq!(
        report,
        SweepReport {
            rows: 0,
            files: 0,
            temps: 1
        }
    );
    assert!(!stale.exists());
    assert!(fresh.exists());
}

/// A14: only regular files, never through a link, and only the names the
/// module writes: a symlink, a directory and any other name stay, however
/// old.
#[test]
fn the_sweep_leaves_links_directories_and_other_names_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let now = SystemTime::now() + 2 * HOUR;
    let dir = files(&db);
    let hash = sha(b"x");
    let outside = plant(&dir.parent().unwrap().join("outside"), "target", b"keep");
    let links = [hash.clone(), temp_name(&hash, "0123456789abcdef")];
    for name in &links {
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join(name)).unwrap();
    }
    let directories = [sha(b"y"), temp_name(&sha(b"y"), "0123456789abcdef")];
    for name in &directories {
        std::fs::create_dir(dir.join(name)).unwrap();
    }
    // The uppercase names are of another hash: on a case-insensitive file
    // system (macOS) they would be the links' names.
    let upper = sha(b"z");
    let others = [
        "notes.txt".to_string(),
        upper.to_uppercase(),
        format!("{hash}x"),
        format!("{hash}.tmp"),
        format!(".{hash}.tmp"),
        temp_name(&hash, "0123456789abcde"),
        temp_name(&hash, "0123456789abcdef0"),
        temp_name(&upper, "0123456789ABCDEF"),
        format!("{}.bak", temp_name(&hash, "0123456789abcdef")),
        temp_name(&hash, "0123456789abcdef")[1..].to_string(),
    ];
    for name in &others {
        plant(&dir, name, b"other");
    }

    let report = store.sweep_attachments(now).unwrap();
    assert_eq!(report, SweepReport::default());
    for name in &links {
        let meta = std::fs::symlink_metadata(dir.join(name)).unwrap();
        assert!(meta.file_type().is_symlink(), "{name}");
    }
    assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    for name in directories.iter().chain(&others) {
        assert!(std::fs::symlink_metadata(dir.join(name)).is_ok(), "{name}");
    }
}

/// With no `attachments/` yet (nothing sent), or an in-memory store, a
/// sweep is the row pass alone.
#[test]
fn a_sweep_with_no_files_is_the_row_pass_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    assert!(!files(&db).exists());
    assert_eq!(
        store.sweep_attachments(SystemTime::now()).unwrap(),
        SweepReport::default()
    );
    let memory = Store::open_in_memory().unwrap();
    assert_eq!(
        memory.sweep_attachments(SystemTime::now()).unwrap(),
        SweepReport::default()
    );
}

/// Decision 9: a turn abandoned before plan 9a's clean-up existed left
/// its image's row, which nothing shows, and its file. One sweep removes
/// both (the file once past the grace).
#[test]
fn a_row_left_by_a_turn_abandoned_before_the_clean_up_goes_with_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let bytes = png(13, 100);
    active(&store, "s1");
    prompt_in(&store, "s1", "t1", vec![image(&bytes)]);
    // `abandon_turn` as it was before plan 9a: the turn goes (its links
    // with it), its image's row stays.
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         UPDATE sessions SET open_turn_id = NULL WHERE id = 's1' AND open_turn_id = 't1';
         DELETE FROM turns WHERE turn_id = 't1';",
    )
    .unwrap();
    let path = files(&db).join(sha(&bytes));
    assert_eq!(rows(&db), vec![(store.owner_id().to_string(), sha(&bytes))]);
    assert!(path.exists());

    let report = store.sweep_attachments(SystemTime::now() + 2 * HOUR).unwrap();
    assert_eq!(
        report,
        SweepReport {
            rows: 1,
            files: 1,
            temps: 0
        }
    );
    assert_eq!(rows(&db), vec![]);
    assert!(!path.exists());
    assert!(store.attachment(&sha(&bytes)).unwrap().is_none());
}

/// Decision 9: an image `save_images` has just written, which no row names
/// until `open_prompt` records it, is kept by the grace.
#[test]
fn an_image_just_saved_is_kept_until_its_turn_records_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let bytes = png(14, 100);
    let checked = content::check(vec![image(&bytes)]).unwrap();
    store.save_images(&checked.images).unwrap();
    let path = files(&db).join(sha(&bytes));
    assert!(path.exists());

    let within = SystemTime::now() + HOUR - Duration::from_secs(60);
    assert_eq!(store.sweep_attachments(within).unwrap(), SweepReport::default());
    assert!(path.exists(), "an image just saved was swept");
    active(&store, "s1");
    assert!(store.open_prompt("s1", "t1", &checked).unwrap());
    assert_eq!(
        store.sweep_attachments(SystemTime::now() + 2 * HOUR).unwrap(),
        SweepReport::default()
    );
    assert!(path.exists());
}

/// Decision 9: `save_images` refreshes the mtime of an image it finds
/// already stored, so the grace covers it until `open_prompt` records it.
#[test]
fn saving_an_image_already_stored_refreshes_it_for_the_grace() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let bytes = png(7, 100);
    let checked = content::check(vec![image(&bytes)]).unwrap();
    store.save_images(&checked.images).unwrap();
    let path = files(&db).join(sha(&bytes));
    set_mtime(&path, SystemTime::now() - 2 * HOUR);

    let before = SystemTime::now() - Duration::from_secs(1);
    store.save_images(&checked.images).unwrap();
    assert!(mtime(&path) >= before, "{:?}", mtime(&path));
    // No row names it yet: only the grace keeps it.
    assert_eq!(
        store.sweep_attachments(SystemTime::now()).unwrap(),
        SweepReport::default()
    );
    assert!(path.exists());
    assert_eq!(store.sweep_attachments(SystemTime::now() + 2 * HOUR).unwrap().files, 1);
    assert!(!path.exists());
}

/// An `AppState` on the store of `dir`.
fn state(store: Store, db: &Path) -> hennery_sessions::AppState {
    use hennery_kernel::hosts::Hosts;
    use hennery_kernel::operator::Operator;
    hennery_sessions::AppState::new(store, Hosts::open(db).unwrap(), Operator::open(db).unwrap())
}

/// Plant an image file no row names, past the grace.
fn plant_orphan(db: &Path, seed: u8) -> PathBuf {
    let bytes = png(seed, 100);
    let path = plant(&files(db), &sha(&bytes), &bytes);
    set_mtime(&path, SystemTime::now() - 2 * HOUR);
    path
}

/// Wait, polling, until `path` is gone.
async fn gone(path: &Path, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while path.exists() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Decision 9: the collector sweeps when it starts, not an interval later
/// (the interval here is the hourly default), and the task ends on
/// shutdown.
#[tokio::test]
async fn the_sweep_runs_at_startup_and_ends_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let path = plant_orphan(&db, 8);
    let state = state(store, &db);
    assert_eq!(state.sweep_interval, hennery_sessions::sweep::INTERVAL);

    let task = hennery_sessions::sweep::after_startup(&state);
    gone(&path, "the startup sweep did not run").await;
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the sweep task ended on shutdown")
        .unwrap();
}

/// Decision 9: after the sweep at startup, the collector sweeps again
/// every `sweep_interval` (hourly; shortened here): a file orphaned after
/// the first sweep goes in a later one.
#[tokio::test]
async fn the_sweep_runs_again_every_interval() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let first = plant_orphan(&db, 9);
    let mut state = state(store, &db);
    state.sweep_interval = Duration::from_millis(100);

    let task = hennery_sessions::sweep::after_startup(&state);
    gone(&first, "the startup sweep did not run").await;
    for seed in [10, 11] {
        let later = plant_orphan(&db, seed);
        gone(&later, "no sweep ran after the first").await;
    }
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .expect("the sweep task ended on shutdown")
        .unwrap();
}

/// Decision 9: serving starts the sweep, as it starts the offline watch.
#[tokio::test]
async fn serving_starts_the_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let (store, db) = file_store(dir.path());
    let path = plant_orphan(&db, 12);
    let state = state(store, &db);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();

    let served = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
    gone(&path, "serving did not start the sweep").await;
    state.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(20), served)
        .await
        .expect("serving ended on shutdown")
        .unwrap()
        .unwrap();
}
