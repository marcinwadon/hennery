# Delete and purge (plan 9b): the orphan sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Image files and rows that nothing references are removed (6a's hand-on; ACP core §4.10, §7).
- **When:** at collector startup and then hourly.
- **What goes:**
  - the owner's `attachments` rows with no turn or event referencing them;
  - files in `attachments/` that no row of any owner names, after a one-hour grace;
  - `.tmp` files left by a crash, after the same grace.
- **How:** never through a link, never a name the store does not make, and in batches under the store's lock, so it never races a prompt or a delete.

This is the second of plan 9's parts. It covers what 9a hands on, a crash between a delete's commit and its file removal, and what 6a hands on, files orphaned by abandoned or lost-race turns.

**Architecture:**
- **`hennery-sessions/src/sweep.rs`:**
  - `after_startup` starts one sweep at once, then one every `AppState::sweep_interval` (an hour by default), until shutdown;
  - `GRACE` (an hour), `sweep_file` and `Swept`.
- **`Store::sweep_attachments(now, &CancellationToken) -> Swept`:**
  - the row pass: one owner-filtered `DELETE` of unreferenced rows;
  - the file pass: lists the directory outside the lock, then under the lock, in batches of 32, re-checks each name before unlinking it. A name must be a hash (`is_sha256`) or `write`'s temporary name (`is_temp`); the entry must be a regular file (`symlink_metadata`), past the grace, and named by no row of any owner (9a's `shared_files::hash_named_by_any_owner`, the one cross-owner read);
  - shutdown is checked before every batch.
- **`attachments.rs`:**
  - `refresh`: `save_images` touches the mtime of an image it finds already stored, so the grace covers it until its turn records it;
  - regular files only: never a link, a directory or a FIFO;
  - `is_temp` and `temp_name`.
- **Wiring:** `serve_on`, and the collector's `main.rs`, which calls `serve_all`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** ACP core [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md):
- §4.10: "attachment files no longer referenced by any event are removed";
- §15, decision 6a: "Images live as long as their session".

It builds on plan [9a](2026-10-15-session-delete.md) (`turn_attachments`, `hash_named_by_any_owner`, `attachments_by_hash`) and on 6a's hand-on: "An image whose turn was abandoned ... keeps its file and row. A sweep, with the delete above, removes them; so leftover `.tmp` files from a crash." Every anchor was taken from `main` at `350ec96`.

**Status:** executed 2026-10-02 (see "Execution status"). The plan-9 security review covered it (decision 9, A6, A14), and its scoped re-confirmation answered "confirmed with notes".

**How the code blocks were made and checked:**
- Every block below was generated from the reviewed commits, as diffs from `350ec96`.
- The plan was replayed from its own text onto `350ec96`, and the tree matched the task's commit byte for byte (`replay.py`, every block applied).

## Execution status (2026-10-02)

**Executed** on `main` at `350ec96`:
- An opus implementer built it, then took over a killed agent's work in progress and replaced it.
- An opus review: "ready after fixes".
- A fix round, then a rebase onto `350ec96`.

| Area | As built | Why |
|---|---|---|
| The killed agent's WIP | Kept its core (the two passes, `is_temp`, `refresh`, `after_startup`). Added the wiring (it was called nowhere), the injectable interval, and four tests. | The sweep never ran. |
| Review finding 1 (Important) | `every_batch_of_files_is_swept` plants 70 orphans, three batches. | A sweep that stopped after the first batch passed every test. |
| Review findings 2–5 | Shutdown is checked before every batch. `refresh` follows no link and opens no FIFO. A comment marks the row-and-link transaction. `sweep_file` moved to sweep.rs. | Stopping on shutdown, defence in depth, and the code's own layout. |
| The row pass has no grace | Safe because `open_turn_with` inserts an image's row and its link in one transaction under the store's lock. The comment there says so. | A later writer inserting rows without their link would lose them to the sweep. |

Checks:
- The five checks passed (1085 tests on `350ec96`, from 1068).
- Then rebased onto `d2d2894` (#65, #74: the host's Node first run, CI concurrency) and `522e802` and `ce47137` (#75, #77, #78, #69, #70: the gateway's internal HTTP and store, the view fold, doctor and host-offline fixes) with no conflict; the five checks passed on `ce47137`, 1221 tests. The blocks are generated and replayed against `350ec96`; neither PR touches a file this plan changes.
- Every delete statement and side-effect line was revert-probed (Step 5).
- The run was macOS only. Ubuntu CI is the Linux check.

## Scope

Decision 9 of plan 9, with A6 and A14. That is **1 task**.

**Out of scope:**
- A sweep of rows with a grace. Rows are never written without their link.
- Symlinks already at an image's name. `write`'s `exists()` follows them. The directory is the collector's own (0700), so this is recorded only.

## Decisions this plan makes where the spec is silent

The plan-9 security review confirmed these on the maintainer's behalf.

1. **When.** The sweep runs at startup and then every hour (`AppState::sweep_interval`).
2. **What is a candidate.** A name that is a hash, or `write`'s temporary name, and a regular file whose mtime is over an hour old.
3. **What keeps a file.** Any owner's row that names it. Files are shared by hash, so 9a's one exempted read decides.
4. **The race with a prompt.** `save_images` refreshes an existing file's mtime, so the grace covers the time until `open_prompt`. 9a's re-write in `open_turn_with` covers the rest.
5. **Lock time.** The listing is taken outside the lock. Removal happens in batches of 32 under it, with every check repeated just before the unlink.
6. **Shutdown.** It is checked before every batch. The row pass always runs.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`.
- The five checks pass.
- No new crates. No wire types change.
- Every query names the owner. The only cross-owner read stays 9a's exempted one.
- No production test hooks. No Linux-only code.

## Review Focus

1. **Nothing referenced is removed.**
   - Expected: rows and files of any owner that something names stay.
   - Tests: `a_file_another_owners_row_names_stays`, `the_sweep_deletes_the_owners_rows_nothing_references`.
2. **Nothing that is not ours is removed.**
   - Expected: links, directories and other names stay.
   - Tests: `the_sweep_leaves_links_directories_and_other_names_alone`.
3. **A fresh image survives.**
   - Expected: an image just saved stays until its turn records it.
   - Tests: `an_image_just_saved_is_kept_until_its_turn_records_it`, `saving_an_image_already_stored_refreshes_it_for_the_grace`.
4. **Every batch and shutdown.**
   - Tests: `every_batch_of_files_is_swept`, `a_cancelled_sweep_removes_no_further_files`, `the_sweep_stops_on_the_collectors_shutdown`.

**Reading the steps:** as in plan 9a.

---

### Task 1: The sweep

**Files:**
- Create: `crates/hennery-sessions/src/sweep.rs`, `crates/hennery-sessions/tests/sweep.rs`
- Modify: `crates/hennery-sessions/src/{store,attachments,lib}.rs`, `crates/hennery/src/main.rs`

- [ ] **Step 1: Write the failing tests**

  Create `crates/hennery-sessions/tests/sweep.rs`:

  ```rust
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
  use tokio_util::sync::CancellationToken;

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

  /// One sweep at `now` that nothing cancels.
  fn sweep(store: &Store, now: SystemTime) -> SweepReport {
      store.sweep_attachments(now, &CancellationToken::new()).unwrap()
  }

  /// `count` image files no row names, past the grace of a sweep two hours
  /// ahead, each of its own hash.
  fn plant_orphans(db: &Path, count: u8) -> Vec<PathBuf> {
      (0..count)
          .map(|seed| {
              let bytes = png(seed, 100);
              plant(&files(db), &sha(&bytes), &bytes)
          })
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

      let report = sweep(&store, SystemTime::now());
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

      let report = sweep(&store, now);
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

      let report = sweep(&store, SystemTime::now() + 2 * HOUR);
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

      let report = sweep(&store, now);
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

      let report = sweep(&store, now);
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

  /// A14: the file pass works in batches under the store's lock, and every
  /// batch is swept, not just the first: more files than two batches hold
  /// all go.
  #[test]
  fn every_batch_of_files_is_swept() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      plant_orphans(&db, 70);

      let report = sweep(&store, SystemTime::now() + 2 * HOUR);
      assert_eq!(report.files, 70, "{report:?}");
      let left: Vec<_> = std::fs::read_dir(files(&db))
          .unwrap()
          .map(|e| e.unwrap().file_name())
          .collect();
      assert!(left.is_empty(), "{left:?}");
  }

  /// A sweep checks its token before each batch: once shutdown has
  /// cancelled it, no further file is removed. The row pass still runs.
  #[test]
  fn a_cancelled_sweep_removes_no_further_files() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let planted = plant_orphans(&db, 70);
      let cancel = CancellationToken::new();
      cancel.cancel();

      let report = store.sweep_attachments(SystemTime::now() + 2 * HOUR, &cancel).unwrap();
      assert_eq!(report, SweepReport::default());
      assert!(planted.iter().all(|path| path.exists()));
  }

  /// With no `attachments/` yet (nothing sent), or an in-memory store, a
  /// sweep is the row pass alone.
  #[test]
  fn a_sweep_with_no_files_is_the_row_pass_alone() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      assert!(!files(&db).exists());
      assert_eq!(sweep(&store, SystemTime::now()), SweepReport::default());
      let memory = Store::open_in_memory().unwrap();
      assert_eq!(sweep(&memory, SystemTime::now()), SweepReport::default());
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

      let report = sweep(&store, SystemTime::now() + 2 * HOUR);
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
      assert_eq!(sweep(&store, within), SweepReport::default());
      assert!(path.exists(), "an image just saved was swept");
      active(&store, "s1");
      assert!(store.open_prompt("s1", "t1", &checked).unwrap());
      assert_eq!(sweep(&store, SystemTime::now() + 2 * HOUR), SweepReport::default());
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
      assert_eq!(sweep(&store, SystemTime::now()), SweepReport::default());
      assert!(path.exists());
      assert_eq!(sweep(&store, SystemTime::now() + 2 * HOUR).files, 1);
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

  /// A sweep is cancelled by the collector's shutdown: one that starts after
  /// it has fired removes no files, and the task ends.
  #[tokio::test]
  async fn the_sweep_stops_on_the_collectors_shutdown() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let planted = plant_orphans(&db, 70);
      for path in &planted {
          set_mtime(path, SystemTime::now() - 2 * HOUR);
      }
      let state = state(store, &db);
      state.shutdown.cancel();

      let task = hennery_sessions::sweep::after_startup(&state);
      tokio::time::timeout(Duration::from_secs(20), task)
          .await
          .expect("the sweep task ended on shutdown")
          .unwrap();
      assert!(planted.iter().all(|path| path.exists()));
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
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-sessions --locked --test sweep`
Expected: FAIL to compile: `no method named sweep_attachments found for struct Store`, `unresolved import hennery_sessions::sweep`.

- [ ] **Step 3: The sweep**

  In `crates/hennery-sessions/src/attachments.rs`, replace:

  ```rust

  fn path(dir: &Path, sha256: &str) -> std::io::Result<PathBuf> {
  ```

  with:

  ```rust

  /// The temporary file `write` writes `sha256` to before its rename:
  /// `.<sha256>.<16 hex digits>.tmp`, hidden, and unique per write.
  fn temp_name(sha256: &str, random: [u8; 8]) -> String {
      format!(".{sha256}.{}.tmp", hex::encode(random))
  }

  /// `name` is a temporary file as `write` names one, which a crash may
  /// leave (plan 9b): the only other name the sweep removes.
  pub fn is_temp(name: &str) -> bool {
      let is_hex = |s: &str| s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
      name.strip_prefix('.')
          .and_then(|rest| rest.strip_suffix(".tmp"))
          .and_then(|rest| rest.split_once('.'))
          .is_some_and(|(sha256, random)| is_sha256(sha256) && random.len() == 16 && is_hex(random))
  }

  /// Refresh the mtime of the file stored as `sha256` in `dir`, if there is
  /// one (plan 9b): whether there was. The sweep removes a file no row names
  /// only once its mtime is older than its grace.
  ///
  /// Only a regular file is refreshed: anything else under that name is
  /// not one, and opening it could follow a link out of the directory or
  /// block on a FIFO.
  pub fn refresh(dir: &Path, sha256: &str) -> std::io::Result<bool> {
      let path = path(dir, sha256)?;
      match std::fs::symlink_metadata(&path) {
          Ok(meta) if meta.file_type().is_file() => {}
          Ok(_) => return Ok(false),
          Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
          Err(err) => return Err(err),
      }
      match std::fs::File::open(path) {
          Ok(file) => file.set_modified(std::time::SystemTime::now()).map(|()| true),
          Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
          Err(err) => Err(err),
      }
  }

  fn path(dir: &Path, sha256: &str) -> std::io::Result<PathBuf> {
  ```

  In `crates/hennery-sessions/src/attachments.rs`, replace:

  ```rust
      let temp = dir.join(format!(
          ".{sha256}.{}.tmp",
          hex::encode(hennery_kernel::secret::random_bytes::<8>())
      ));
  ```

  with:

  ```rust
      let temp = dir.join(temp_name(sha256, hennery_kernel::secret::random_bytes::<8>()));
  ```

  In `crates/hennery-sessions/src/attachments.rs`, replace:

  ```rust
      }

      #[test]
      fn a_file_is_written_whole_private_and_once() {
  ```

  with:

  ```rust
      }

      /// Plan 9b: the sweep removes a leftover temporary file by its name,
      /// so the name `write` makes is the only one it matches.
      #[test]
      fn only_the_names_write_makes_are_temporary_files() {
          let random = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
          let name = temp_name(SHA, random);
          assert_eq!(name, format!(".{SHA}.0123456789abcdef.tmp"));
          assert!(is_temp(&name));
          for other in [
              SHA.to_string(),
              name[1..].to_string(),
              format!("{name}.bak"),
              format!(".{SHA}.tmp"),
              format!(".{SHA}.0123456789abcde.tmp"),
              format!(".{SHA}.0123456789abcdef0.tmp"),
              format!(".{SHA}.0123456789ABCDEF.tmp"),
              format!(".{}.0123456789abcdef.tmp", SHA.to_uppercase()),
              format!(".{}.0123456789abcdef.tmp", &SHA[1..]),
              format!(".{SHA}.0123456789abcdef.tmq"),
              format!(".{SHA}/0123456789abcdef.tmp"),
          ] {
              assert!(!is_temp(&other), "{other}");
          }
      }

      /// Plan 9b: `refresh` touches a regular file only. It never follows a
      /// link out of the directory, and never opens a FIFO, which would
      /// block it until a writer came.
      #[test]
      fn only_a_regular_file_is_refreshed() {
          let data = tempfile::tempdir().unwrap();
          let dir = data.path().join(DIR);
          let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
          assert!(!refresh(&dir, SHA).unwrap(), "a missing file was refreshed");
          write(&dir, SHA, b"test").unwrap();
          std::fs::File::open(dir.join(SHA)).unwrap().set_modified(old).unwrap();
          assert!(refresh(&dir, SHA).unwrap());
          assert!(std::fs::metadata(dir.join(SHA)).unwrap().modified().unwrap() > old);

          let outside = data.path().join("outside");
          std::fs::write(&outside, b"keep").unwrap();
          std::fs::File::open(&outside).unwrap().set_modified(old).unwrap();
          let link = "0".repeat(64);
          std::os::unix::fs::symlink(&outside, dir.join(&link)).unwrap();
          assert!(!refresh(&dir, &link).unwrap(), "a symlink was refreshed");
          assert_eq!(std::fs::metadata(&outside).unwrap().modified().unwrap(), old);

          let directory = "1".repeat(64);
          std::fs::create_dir(dir.join(&directory)).unwrap();
          assert!(!refresh(&dir, &directory).unwrap(), "a directory was refreshed");

          let fifo = "2".repeat(64);
          let made = std::process::Command::new("mkfifo")
              .arg(dir.join(&fifo))
              .status()
              .unwrap();
          assert!(made.success());
          let (sent, received) = std::sync::mpsc::channel();
          let (in_dir, name) = (dir.clone(), fifo.clone());
          std::thread::spawn(move || sent.send(refresh(&in_dir, &name).map_err(|e| e.to_string())));
          let refreshed = received.recv_timeout(std::time::Duration::from_secs(20));
          if refreshed.is_err() {
              // Release the thread blocked in `open`.
              let _ = std::fs::OpenOptions::new().write(true).open(dir.join(&fifo));
          }
          assert_eq!(refreshed.expect("refresh blocked on a FIFO"), Ok(false));
      }

      #[test]
      fn a_file_is_written_whole_private_and_once() {
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  pub mod store;
  pub mod ws;
  ```

  with:

  ```rust
  pub mod store;
  pub mod sweep;
  pub mod ws;
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      pub push: Push,
  }
  ```

  with:

  ```rust
      pub push: Push,
      /// How often the attachments are swept after the sweep at startup
      /// (plan 9b decision 9).
      pub sweep_interval: Duration,
  }
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
              push: Push::detached(),
          }
  ```

  with:

  ```rust
              push: Push::detached(),
              sweep_interval: sweep::INTERVAL,
          }
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      offline::after_startup(&state);
      serve_all(listeners, router(state.clone()), state.shutdown.clone()).await
  ```

  with:

  ```rust
      offline::after_startup(&state);
      sweep::after_startup(&state);
      serve_all(listeners, router(state.clone()), state.shutdown.clone()).await
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust

  /// A stored image (plan 6a), for `GET /api/attachments/{sha256}`.
  ```

  with:

  ```rust

  /// What one sweep removed (plan 9b): the owner's rows nothing of theirs
  /// showed, the image files no row of any owner named, and the leftover
  /// temporary files.
  #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
  pub struct SweepReport {
      pub rows: u64,
      pub files: u64,
      pub temps: u64,
  }

  /// A stored image (plan 6a), for `GET /api/attachments/{sha256}`.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      Ok(dropped)
  }

  /// Close an open turn that the host will never end, as `interrupted`.
  ```

  with:

  ```rust
      Ok(dropped)
  }

  /// How many files a sweep looks at under one hold of the store's lock
  /// (A14), so a prompt waits for a few files, not the whole directory.
  const SWEEP_BATCH: usize = 32;

  /// Close an open turn that the host will never end, as `interrupted`.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              for image in images {
                  tx.execute(
  ```

  with:

  ```rust
              for image in images {
                  // The row and its turn's link (`link_turn_attachments`) go in
                  // this one transaction: the sweep gives rows no grace, and
                  // deletes one no turn or event links.
                  tx.execute(
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// An image already stored is kept as it is.
  ```

  with:

  ```rust
      /// An image already stored is kept as it is, its mtime refreshed: until
      /// `open_prompt` records it, no row may name it, and only the sweep's
      /// grace keeps it (plan 9b decision 9).
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              crate::attachments::write(dir, &image.sha256, &image.bytes)
                  .with_context(|| format!("store attachment {}", image.sha256))?;
  ```

  with:

  ```rust
              let refreshed = crate::attachments::refresh(dir, &image.sha256)
                  .with_context(|| format!("refresh attachment {}", image.sha256))?;
              if !refreshed {
                  crate::attachments::write(dir, &image.sha256, &image.bytes)
                      .with_context(|| format!("store attachment {}", image.sha256))?;
              }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  tracing::warn!(%sha256, "an unreferenced attachment's file was left: {err:#}");
              }
          }
      }

      /// Delete a session (ACP core §4.10; plan 9a decisions 1, 5 and 6), in
  ```

  with:

  ```rust
                  tracing::warn!(%sha256, "an unreferenced attachment's file was left: {err:#}");
              }
          }
      }

      /// Sweep the attachments (plan 9b decision 9, A14), at `now`:
      /// - the owner's rows that no turn and no event of theirs shows go;
      /// - then `attachments/` is listed without the store's lock, and only
      ///   two kinds of name are taken from it: an image's (`is_sha256`) and
      ///   a write's temporary file (`is_temp`). Under the lock, a few at a
      ///   time, each is looked at again and removed if it is a regular file
      ///   (never through a link), its mtime is older than `sweep::GRACE`,
      ///   and, for an image, no row of any owner names it.
      ///
      /// The checks are made under the lock so they hold at the unlink:
      /// `open_prompt` records an image under it (and re-writes a file gone
      /// meanwhile, decision 6), and `save_images`, which does not take it,
      /// refreshes a file's mtime first. A file that cannot be looked at or
      /// removed is logged and left for the next sweep. Once `cancel` fires
      /// (the collector's shutdown), no further batch is begun.
      pub fn sweep_attachments(
          &self,
          now: std::time::SystemTime,
          cancel: &tokio_util::sync::CancellationToken,
      ) -> Result<SweepReport> {
          let rows = self.conn().execute(
              "DELETE FROM attachments WHERE owner_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM turn_attachments
                       WHERE turn_attachments.owner_id = ?1 AND turn_attachments.sha256 = attachments.sha256)
                   AND NOT EXISTS (SELECT 1 FROM event_attachments
                       WHERE event_attachments.owner_id = ?1 AND event_attachments.sha256 = attachments.sha256)",
              [&self.owner],
          )?;
          let mut report = SweepReport {
              rows: rows as u64,
              ..SweepReport::default()
          };
          let Some(dir) = self.attachments.as_deref() else {
              return Ok(report);
          };
          let listed = match std::fs::read_dir(dir) {
              Ok(listed) => listed,
              // Nothing sent yet.
              Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(report),
              Err(err) => return Err(err).context("list the attachment files"),
          };
          let mut names = Vec::new();
          for entry in listed {
              let Ok(name) = entry.context("list the attachment files")?.file_name().into_string() else {
                  continue;
              };
              if crate::attachments::is_sha256(&name) || crate::attachments::is_temp(&name) {
                  names.push(name);
              }
          }
          for batch in names.chunks(SWEEP_BATCH) {
              if cancel.is_cancelled() {
                  break;
              }
              let conn = self.conn();
              for name in batch {
                  match crate::sweep::sweep_file(&conn, dir, name, now) {
                      Ok(Some(crate::sweep::Swept::Image)) => report.files += 1,
                      Ok(Some(crate::sweep::Swept::Temp)) => report.temps += 1,
                      Ok(None) => {}
                      Err(err) => tracing::warn!(%name, "an attachment file was not swept: {err:#}"),
                  }
              }
          }
          Ok(report)
      }

      /// Delete a session (ACP core §4.10; plan 9a decisions 1, 5 and 6), in
  ```

  Create `crates/hennery-sessions/src/sweep.rs`:

  ```rust
  //! The orphan sweep (plan 9b decision 9, A14).
  //!
  //! Images outlive what showed them: a turn abandoned or never delivered,
  //! a prompt that lost its race for the turn (plan 6a), and a session
  //! delete that crashed between its commit and its files (plan 9a decision
  //! 7). A crashed write leaves a `.tmp` file (plan 6a decision 7). When the
  //! collector starts and then every `AppState::sweep_interval` (`INTERVAL`,
  //! hourly), `Store::sweep_attachments` deletes the owner's rows nothing of
  //! theirs shows, then the files no row of any owner names and the `.tmp`
  //! files, each only once its mtime is older than `GRACE`.

  use crate::AppState;
  use anyhow::Result;
  use rusqlite::Connection;
  use std::path::Path;
  use std::time::Duration;

  /// How old a file must be before a sweep removes it: an image is saved
  /// before its turn records it (`save_images`, then `open_prompt`), and a
  /// `.tmp` file may be a write still running.
  pub const GRACE: Duration = Duration::from_secs(60 * 60);

  /// How often the collector sweeps, after its sweep at startup: the
  /// default of `AppState::sweep_interval`.
  pub const INTERVAL: Duration = Duration::from_secs(60 * 60);

  /// Called once when the collector starts: sweep now, then every
  /// `state.sweep_interval`, until `state.shutdown`, which also stops a
  /// sweep that is running before its next batch. A sweep that fails is
  /// logged, and the next one runs as planned.
  pub fn after_startup(state: &AppState) -> tokio::task::JoinHandle<()> {
      let state = state.clone();
      tokio::spawn(async move {
          loop {
              let (store, cancel) = (state.store.clone(), state.shutdown.clone());
              let swept =
                  tokio::task::spawn_blocking(move || store.sweep_attachments(std::time::SystemTime::now(), &cancel))
                      .await;
              match swept {
                  Ok(Ok(report)) => tracing::info!(
                      rows = report.rows,
                      files = report.files,
                      temps = report.temps,
                      "attachments swept"
                  ),
                  Ok(Err(err)) => tracing::error!("the attachment sweep failed: {err:#}"),
                  Err(err) => tracing::error!("the attachment sweep panicked: {err}"),
              }
              tokio::select! {
                  _ = tokio::time::sleep(state.sweep_interval) => {}
                  _ = state.shutdown.cancelled() => return,
              }
          }
      })
  }

  /// What `sweep_file` removed.
  pub(crate) enum Swept {
      Image,
      Temp,
  }

  /// Remove `name` from `dir` if it is still a regular file older than the
  /// grace at `now` and, for an image, no row of any owner names it (A6);
  /// under the store's lock (`Store::sweep_attachments`).
  pub(crate) fn sweep_file(
      conn: &Connection,
      dir: &Path,
      name: &str,
      now: std::time::SystemTime,
  ) -> Result<Option<Swept>> {
      let path = dir.join(name);
      let meta = match std::fs::symlink_metadata(&path) {
          Ok(meta) => meta,
          Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
          Err(err) => return Err(err.into()),
      };
      if !meta.file_type().is_file() {
          return Ok(None);
      }
      // An mtime ahead of `now` is young.
      let old = now.duration_since(meta.modified()?).is_ok_and(|age| age > GRACE);
      if !old {
          return Ok(None);
      }
      let swept = if crate::attachments::is_sha256(name) {
          if crate::shared_files::hash_named_by_any_owner(conn, name)? {
              return Ok(None);
          }
          Swept::Image
      } else {
          Swept::Temp
      };
      match std::fs::remove_file(&path) {
          Ok(()) => Ok(Some(swept)),
          Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
          Err(err) => Err(err.into()),
      }
  }
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      hennery_sessions::offline::after_startup(&state);
      let listeners = listeners
  ```

  with:

  ```rust
      hennery_sessions::offline::after_startup(&state);
      hennery_sessions::sweep::after_startup(&state);
      let listeners = listeners
  ```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-sessions --locked --test sweep --lib`
Expected: PASS. The timing tests (startup, hourly, serving, shutdown) poll with a 20 s deadline. They passed with 4 copies of the binary in parallel, three rounds.

- [ ] **Step 5: Revert-probes**

Each change is made, the named test is run, and the change is restored. Every one was caught:
- the row `DELETE` (made a `SELECT`), each of its two `NOT EXISTS`, and its owner filter: `the_sweep_deletes_the_owners_rows_nothing_references` (the owner audit too, for the filter);
- `symlink_metadata` → `metadata`, the `is_file` check, and the listing's name filter: `the_sweep_leaves_links_directories_and_other_names_alone`;
- the grace check: `an_image_just_saved_is_kept_until_its_turn_records_it` and three more;
- skipping `hash_named_by_any_owner`: `a_file_another_owners_row_names_stays`;
- `remove_file` as a no-op: seven tests;
- `is_temp`'s length check: its unit test;
- `refresh` in `save_images`: `saving_an_image_already_stored_refreshes_it_for_the_grace`;
- `refresh`'s non-regular arm, `metadata` for `symlink_metadata`, directories or FIFOs allowed: `only_a_regular_file_is_refreshed`;
- the loop, the interval, sleeping before the first sweep, the shutdown arm, and the `serve_on` line: the startup, hourly and serving tests;
- `.take(1)` on the batches: `every_batch_of_files_is_swept`;
- the cancel check, and a fresh token in `after_startup`: the two cancellation tests;
- the owner-audit minimum, set one higher: the audit.

Not probed: the `main.rs` line, which no cheap test reaches. It mirrors the probed `serve_on` line.

- [ ] **Step 6: The full checks**

Expected: all pass; **1085 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(sessions): sweep orphaned attachment files at start and hourly"
```

## After this plan

**Obligations this plan hands on:**
- **For teams (umbrella §15):** `hash_named_by_any_owner` stays the one cross-owner read. Per-owner files (6a's O5) would remove it.
- **Symlinks at an image's name** (`write`'s `exists()` follows them): recorded, not fixed. The directory is private.
- **The row pass has no grace.** A future writer must insert an image's row together with its link.

**Not tested here:**
- A cancel landing mid-sweep, between two batches. The tests prove the check runs before every batch.
- The `main.rs` wiring line.
- **Linux:** ubuntu CI is the first run of the FIFO test there.

**Spec amendments:** ACP core §7 and §15: the sweep (startup and hourly, the grace, the candidates, what keeps a file).

Then, in order:
- **(9c) The per-hat purge**
- **(9d) The agent's own transcript on its host**

---

_Generated with Claude AI — please review before distribution._
