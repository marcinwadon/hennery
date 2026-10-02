# Delete and purge (plan 9a): session delete Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The operator can delete a session (umbrella §6.10, ACP core §4.10).
- `DELETE /api/sessions/{id}` needs a fresh step-up. It closes an attached session first, then deletes it.
- What is deleted:
  - the session's events, turns, pending questions, queued answers and catalogue;
  - its images, unless another session still references them;
  - its project recent, unless another session uses that cwd.
- What is left: a scrubbed tombstone row and one `session_deleted` event. The tombstone holds no content.
- A deleted session never comes back. Its host's frames for it are acked and discarded. A host that still runs it is told to close it at its next handshake, or at once if it reconnected during the delete.

This is the first of plan 9's three parts. 9b, the orphan sweep, and 9c, the per-hat purge, follow.

**Architecture:**
- **Sessions store** (`hennery-sessions/src/store.rs`):
  - migration 13:
    - `turn_attachments`, backfilled from `turns.content`;
    - `attachments_by_hash`;
    - triggers that refuse every write for a tombstone;
  - `Store::delete_session(id, Option<&Unattached>) -> Deletion`, one transaction, with a compare-and-set close;
  - the accessors renamed: `find_session`, `find_session_item`, and `session_host`, which also sees tombstones;
  - `tombstones_of`;
  - tombstone guards in every writer.
- **Files:** `shared_files.rs` holds the one cross-owner read (a file is shared by hash), exempt from the owner audit with its reason. Image files are removed after commit, under the store's lock. A missing file is re-written by `open_turn_with`.
- **Kernel** (`db.rs`): `PRAGMA secure_delete = ON`.
- **Host side** (`ws.rs`):
  - a frame for this host's tombstone is acked and stores nothing;
  - `reconcile_host` closes a listed tombstone;
  - after `mark_ready`, a tombstone the reconciliation missed is closed too.
- **Route** (`api.rs`):
  - `DELETE /api/sessions/{id}`, with step-up layered on the method alone;
  - `GET …/events` and the session stream answer 404 for a deleted session;
  - an open stream ends after `session_deleted`.
- **Tests:**
  - the store: the schema walk, the scrub, raw bytes, images, recents, the triggers and the bulk functions;
  - over a scripted host: resurrection, `not_attached`, every delete path over HTTP, the stream;
  - unit tests for the reconnect race.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md) and ACP core [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md):
- umbrella §6.10: "Delete a session removes its events, attachments, turns, pending requests and queued answers."
- ACP core §4.10: "`DELETE /api/sessions/{id}` (step-up required) closes an attached session first, then deletes its events, turns, pending rows and answer queue entries; attachment files no longer referenced by any event are removed. A `session_deleted` tombstone event (no content) remains."

It builds on the executed plans A and B1 (reconciliation and resume), 5c and 5d (hats), and 6a (images). It takes up 6a's hand-on: a file is removed only when no reference to its hash remains, counting a turn's own content. Every anchor was taken from `main` at `fc00485`.

**Status:** executed 2026-10-02 (see "Execution status"); amended after the security review.

The security review of 2026-10-02 covered all of plan 9 (9a–9c). It approved after amendments, and its scoped re-confirmation answered "confirmed with notes". The reviewer was an opus model, reviewing on the maintainer's behalf.

**How the code blocks were made and checked:**
- Every block below was generated from the reviewed task commits, as diffs from `fc00485`.
- The plan was replayed from its own text onto `fc00485`, task by task, and the tree matched each task's commit byte for byte (`replay.py`, every block applied).

## Execution status (2026-10-02)

**Executed** on `main` at `fc00485`. The code was built task by task by opus implementers, each followed by an opus review. A whole-branch review followed, and the branch was then rebased from `11834e6` onto `4031ba0` (only a doc comment conflicted), and again onto `ef75d4f`: plan 10b had renamed `ingest` to `ingest_fact`, returning `Ingested`, so the tombstone's early return became `Ingested { events: [], edge: None }`; the push lane's deferred notices stay after `ws::ready`. Then onto `4f71aed`, where 10b-iii had taken the store's migration 11, and finally onto `fc00485`, where #67 (a stream's 404 for an unknown session, its replay in pages) had taken migration 12. 9a's is 13, and the backfill test finds it by its text, not its index. #67's stream and 9a's merge: the 404 reads `find_session`; the stream ends after `session_deleted`, live or in a page, through an end mark that `replay_then_follow` now carries (generic over its items). Last, onto `1ab10ab` (#68, the host's process group) with no conflict; the five checks passed there, 1068 tests. The blocks below are generated and replayed against `fc00485`; #68 also changed `e2e.rs` and the ACP core spec, both of which merged with no conflict.

| Area | As built | Why |
|---|---|---|
| Task 1 review (approve with fixes) | A listed tombstone gets `close_session` at reconcile. ws.rs checks a frame's session with `session_host`, which sees tombstones. The WAL checkpoint runs after the lock is released. The writers answer a tombstone as not found. `attachments_by_hash` index. `unconfirmed` holds only when the host may still run the session. | Without the first two, the resurrection test could not pass. A tombstone's frames went unacked, and its adapter was never closed. |
| Task 3 (WIP replaced) | A close answered `not_attached` hands the session's state, as read, to the delete's own compare-and-set. | The first version called `close_now` with no check (A4). |
| Whole-branch review (ready after fixes) | After an unconfirmed delete, the route sends `close_session` to a host that is ready. Right after `mark_ready`, ws.rs closes any listed session that is now a tombstone. The reconcile arm is split into `after_reconcile` and `ready`, and the route into `close_through_host` and `finish_delete`, so unit tests can run each ordering. | A delete that judged the host not ready, after its reconcile but before `mark_ready`, left the adapter running until the next reconnect. |
| A8 under a held reader (the parent's question, before merging) | A checkpoint that a reader holds up is never given up on, and the delete never fails for it. Each attempt waits a second for readers. While a reader holds the WAL, the debt is recorded durably beside the database (`hennery.db-checkpoint-owed`, synced). One thread retries it every second for five minutes, logs once (with no content), then retries every minute until a checkpoint completes, and removes the record. A record found at startup is paid when the store opens. Hennery's own reads are single statements under a lock, so only a reader outside it (a `sqlite3` shell, a backup tool) can hold the WAL for long. Tests: `a_checkpoint_a_reader_held_up_is_retried_once_it_is_gone`; `a_checkpoint_a_reader_holds_past_the_deadline_is_owed_until_it_completes` (held past the deadline, released, clean with no other delete); `a_checkpoint_owed_at_a_restart_is_paid_when_the_store_opens`. | The A8 test failed in two full workspace runs. The cause was the test infrastructure: a count job had built the base commit's crates into this worktree's target (the same crate hashes), so the store's test binary linked a kernel without `secure_delete`. The binary lacked the pragma's string, and a clean rebuild passed repeatedly. The parent's question, whether another connection keeps the WAL from being reset, found the real gap: before this fix, a busy checkpoint left the deleted pages in the WAL until some later checkpoint. |
| Not in a test hook | No production hook was added. `#[cfg(test)]` cannot gate one, because the testkit links the non-test library. | The race tests drive the split steps in order. |

Checks:
- After every task the five checks passed: on `4031ba0`, 986, 988 and 1000 tests from 971; after the rebase onto `fc00485`, 1064 tests from 1032.
- Every delete statement and every side-effect line was revert-probed (Step 5 of each task).
- The run was macOS only. Ubuntu CI is the Linux check.

## Scope

This covers the delete half of umbrella §6.10, with ACP core §4.10's tombstone, and 6a's hand-on that images are counted wherever they are referenced. That is **3 tasks**:
1. the store: delete, the tombstone, the references and the guards;
2. the host side: ack and discard, and close on return;
3. `DELETE /api/sessions/{id}`, the streams, and the reconnect race.

**Out of scope:**
- **The orphan sweep (9b).** A crash between the delete's commit and its file removal leaves files with no row until 9b lands.
- **The per-hat purge (9c).**
- **Revoking the session's gateway tokens (plan 8).** A marked call site waits for it.
- **The agent's own transcript on its host** (Claude's and Codex's session files): sub-plan **9d**, after 9c. Operator delegated, parent decided 2026-10-02: a delete or a per-hat purge also removes the agent's transcript on its host, best effort. It uses the adapter's own delete call if one exists; otherwise only that session's files inside the agent's known session directory, never through a symlink, never outside it, and never a project file. What could not be removed is reported with the delete or purge, and retried when the host reconnects. The tombstone stays as it is here.

## Decisions this plan makes where the spec is silent

The security review of 2026-10-02 confirmed these on the maintainer's behalf. A-numbers are its amendments; R-numbers are the notes from its re-confirmation.

1. **The tombstone is the `sessions` row, scrubbed.**
   - `events.session_id` references `sessions(id)` with foreign keys on, so the spec's tombstone event needs its row.
   - The row keeps its id, owner, host, hat, created and recency fields.
   - Every column that can hold client data is cleared:
     - `cwd` and `agent` are set to `''`;
     - the title, the git fields, the model, the mode, the axes, the agent's session id, the failure reason, the hat rule, the open turn and the activity are set to NULL;
     - the flags are set to 0.
   - Then `lifecycle = 'deleted'`.
   - Tombstones are kept: a host away for months must still be told to close the adapter. There is no undo.
2. **It never comes back, by schema** (A9).
   - Triggers refuse any insert into `events`, `turns`, `pending`, `answer_queue` or `session_catalog` for a tombstone, and any update of its row.
   - The code guards too, so a racing writer gets a typed answer:
     - `collector_event`, `ingest`, `close_in`, `mark_failed`, `record_close_request`, `request_resume`, `reassign_hat` and `catalog`;
     - every multi-session scan (A1).
3. **The accessors are renamed** (fleet rule).
   - `find_session` and `find_session_item` never return a tombstone, so every route answers 404 for one.
   - `session_host` sees tombstones, for ws.rs's ownership check.
   - `deleted` never joins `LIFECYCLES`.
4. **The host side.**
   - A frame for a tombstone of this host is acked and stores nothing, so the host prunes its outbox.
   - `hello_ack.committed` is 0 for a tombstone.
   - `reconcile_host` sends `close_session` for a listed tombstone. The `session_closed` that answers it is discarded, and a `not_attached` answer changes nothing.
5. **`DELETE /api/sessions/{id}` closes first, as `POST …/close` does.**
   - Step-up is checked before anything is read: 403 even for an unknown id.
   - 404 for an unknown or deleted session.
   - A `starting` session on a reachable host gets 409 `starting`.
   - An `active` session on a reachable host gets `close_session`, and the route waits for it. If its delivery is unknown, the answer is 503 `delivery_unknown`, nothing is deleted, and the close stays requested.
   - Anything else is closed collector-side, as the route read it (A4, a compare-and-set in the delete's transaction), and deleted. If the state moved, the answer is 409 with the lifecycle as its code.
   - The answer is 204.
   - The delete publishes `session_deleted`.
6. **What references an image** (6a's hand-on).
   - A reference is a `turn_attachments` row (new: a turn holds its images whether or not its `user_turn` event exists) or an `event_attachments` row.
   - The delete drops the owner's `attachments` rows that are no longer referenced, owner-filtered.
   - After commit, still under the store's lock, it removes each such file, unless any owner's row still names its hash. That check is the one cross-owner read, in `shared_files.rs`, and it is exempt from the owner audit with its reason (A6).
   - `open_turn_with` re-writes a missing file under the same lock, so a prompt re-sending the image cannot lose it.
   - `abandon_turn` does the same clean-up.
7. **SQLite keeps deleted pages** (A8).
   - `secure_delete = ON` for every hennery database.
   - A best-effort `wal_checkpoint(TRUNCATE)` after a delete.
   - A test greps the database files' bytes for a deleted title and cwd.
   - Earlier backups keep the data. A deleted image can stay in the operator's browser cache until the logout change.
8. **Its project recent goes too** (R1–R4).
   - The cwd is read before the scrub.
   - The recent of (host, hat, cwd), matched by exact bytes, is deleted unless another session of that host and hat, not deleted, has that cwd.
9. **`unconfirmed`:** a delete whose session may still run on its host (presumed parked, or `starting` or `active` on a host the route judged unreachable). The route then sends `close_session` if the host is ready by now. ws.rs closes a listed session that is a tombstone by the time it is ready, so every interleaving ends with a close sent.
10. **Streams** (A10): `GET …/events` and `GET /api/stream/sessions/{id}` answer 404 for a deleted session, and an open stream ends after `session_deleted`.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names are prefixed `hennery-`.
- After every task the five checks pass: fmt, both clippy runs, the workspace tests, and `gen --check`.
- **No new crates.** **No wire types change.**
- Every query names the owner. `store.rs` stays in the audit. The one exemption is `shared_files.rs`.
- Every new route is an operator's (`operator_only`).
- **No production test hooks.**
- **No Linux-only code**, and no test that reads another process's state without polling for a positive signal (fleet rule).
- Commits use Conventional Commits, the gmail identity, and are unsigned. Push after every task; never push `main`.

## Review Focus

1. **A session deleted while its host is away, then the host returns.**
   - Expected: its frames are acked and none is stored; `close_session` is sent; the session stays 404 and off the list.
   - Tests: Task 2 `a_session_deleted_while_its_host_was_away_is_closed_and_never_comes_back`.
2. **A delete racing the host's reconnect.**
   - Expected: a close is sent in every ordering.
   - Tests: Task 3 `a_delete_committed_before_the_host_is_ready_is_closed_by_its_connection`, `…after_the_host_is_ready_is_closed_by_the_route`, and `…before_the_reconciliation_is_closed_once`.
3. **Nothing of a session left.**
   - Expected: only the tombstone row and its one event remain, in every table; no title or cwd is left in the database files.
   - Tests: Task 1 `a_deleted_session_leaves_only_its_tombstone_row_and_one_event`, `a_tombstone_is_scrubbed_and_found_by_no_accessor_or_list`, `a_deleted_sessions_title_and_cwd_are_not_left_in_the_database_files`.
4. **Shared images.**
   - Expected: a file stays while any owner's row or any turn or event names it.
   - Tests: Task 1 `an_image_two_sessions_show_stays_until_the_second_is_deleted`, `an_image_only_a_turn_shows_is_still_referenced`, `an_image_file_another_owner_holds_stays_when_the_owners_row_goes`.
5. **Step-up and the close-first rule.**
   - Expected: 403 before anything is read; nothing deleted on 503.
   - Tests: Task 3's step-up row, and `a_delete_whose_close_delivery_is_unknown_deletes_nothing`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-sessions/src/store.rs` | migration 13; `Unattached`, `Deletion`, `delete_session`; tombstone guards; accessors | 1, 3 |
| `crates/hennery-sessions/src/shared_files.rs` | the one cross-owner read | 1 |
| `crates/hennery-sessions/src/attachments.rs` | `remove` | 1 |
| `crates/hennery-kernel/src/db.rs` | `secure_delete` | 1 |
| `crates/hennery-sessions/src/ws.rs` | `session_host`; `after_reconcile`, `ready` | 2, 3 |
| `crates/hennery-sessions/src/api.rs` | `DELETE /api/sessions/{id}`; 404s; stream end | 3 |
| Tests: `crates/hennery-sessions/tests/{store,owner}.rs`; `crates/hennery-testkit/tests/{reconcile,images,step_up,auth,owner_filter,e2e,projects,resolve}.rs` | | 1–3 |

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as text (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

The plan was replayed exactly this way, from its own text, onto `fc00485`.

---

### Task 1: The store: delete, the tombstone, the references

**Files:**
- Create: `crates/hennery-sessions/src/shared_files.rs`
- Modify: `crates/hennery-sessions/src/{store,attachments,lib,api,ws}.rs`, `crates/hennery-kernel/src/db.rs`
- Test: `crates/hennery-sessions/tests/{store,owner}.rs`; `crates/hennery-testkit/tests/{owner_filter,e2e,images,projects,reconcile,resolve}.rs` (the renames)

**Interfaces:**
- Produces:
  - `Store::delete_session`, `Unattached`, `Deletion`;
  - `Store::find_session` and `find_session_item` (renamed from `session` and `session_item`), and `session_host`;
  - `shared_files::hash_named_by_any_owner`.
- Consumes: 6a's `attachments` and `event_attachments`, and 6c's `project_recents`.

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
          // 10b-iii's migration (version 11), 5c's (version 10) and 6b's
          // (version 9) undone first, their indexes on `owner_id` included.
          conn.execute_batch(
              "
  ```

  with:

  ```rust
          // 9a's migration (version 13), 10b-iii's (version 11), 5c's (version
          // 10) and 6b's (version 9) undone first, their indexes on `owner_id`
          // included; the events index (version 12) is made again if missing.
          conn.execute_batch(
              "
              DROP TRIGGER events_of_a_tombstone;
              DROP TRIGGER turns_of_a_tombstone;
              DROP TRIGGER pending_of_a_tombstone;
              DROP TRIGGER answers_of_a_tombstone;
              DROP TRIGGER catalog_of_a_tombstone;
              DROP TRIGGER a_tombstone_stays;
              DROP TABLE turn_attachments;
  ```

  In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
      }
      assert!(store.session("session-a").unwrap().is_some());
      assert_eq!(store.events("session-a", 0, 10).unwrap().len(), 1);
  ```

  with:

  ```rust
      }
      assert!(store.find_session("session-a").unwrap().is_some());
      assert_eq!(store.events("session-a", 0, 10).unwrap().len(), 1);
  ```

  In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
      assert!(store.session("session-a").unwrap().is_some());
  ```

  with:

  ```rust
      assert!(store.find_session("session-a").unwrap().is_some());
  ```

  In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
      assert_eq!(store.session("session-b").unwrap(), None);
      assert_eq!(store.session_item("session-b").unwrap(), None);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("session-b").unwrap(), None);
      assert_eq!(store.find_session_item("session-b").unwrap(), None);
  ```

  In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
      assert_eq!(store.session("session-a").unwrap().unwrap().lifecycle, "parked");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("session-a").unwrap().unwrap().lifecycle, "parked");
  ```

  In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().activity.as_deref(), Some("idle"));
  ```

  with:

  ```rust
      assert_eq!(
          store.find_session("s1").unwrap().unwrap().activity.as_deref(),
          Some("idle")
      );
  ```

  In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
          store.session("s1").unwrap().unwrap().activity.as_deref(),
  ```

  with:

  ```rust
          store.find_session("s1").unwrap().unwrap().activity.as_deref(),
  ```

  In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
      (&store.session("s1").unwrap().unwrap()).into()
  ```

  with:

  ```rust
      (&store.find_session("s1").unwrap().unwrap()).into()
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      started(&store);
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
  ```

  with:

  ```rust
      started(&store);
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(late.is_empty());
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(s.open_turn_id.as_deref(), Some("t2"));
  ```

  with:

  ```rust
      assert!(late.is_empty());
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(s.open_turn_id.as_deref(), Some("t2"));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          .unwrap();
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("parked", None));
  ```

  with:

  ```rust
          .unwrap();
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("parked", None));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s2").unwrap().unwrap().lifecycle, "closed");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s2").unwrap().unwrap().lifecycle, "closed");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          .unwrap();
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.close_requested), ("closed", false));
  ```

  with:

  ```rust
          .unwrap();
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.close_requested), ("closed", false));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&store.close_now("s1").unwrap()), ["operator_closed"]);
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "closed");
      assert!(store.close_now("s1").unwrap().is_empty());
  ```

  with:

  ```rust
      assert_eq!(kinds(&store.close_now("s1").unwrap()), ["operator_closed"]);
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "closed");
      assert!(store.close_now("s1").unwrap().is_empty());
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let lost = store.session("lost").unwrap().unwrap();
  ```

  with:

  ```rust
      let lost = store.find_session("lost").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("pending").unwrap().unwrap().lifecycle, "starting");
      assert_eq!(store.session("other-host").unwrap().unwrap().lifecycle, "starting");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("pending").unwrap().unwrap().lifecycle, "starting");
      assert_eq!(store.find_session("other-host").unwrap().unwrap().lifecycle, "starting");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(r.events[1].body, json!({"turn_id": "t1", "outcome": "interrupted"}));
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.open_turn_id.as_deref()), ("parked", None));
  ```

  with:

  ```rust
      assert_eq!(r.events[1].body, json!({"turn_id": "t1", "outcome": "interrupted"}));
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.open_turn_id.as_deref()), ("parked", None));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
  ```

  with:

  ```rust
      assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(r.events.is_empty());
      assert_eq!(
          store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
          Some("t1")
      );
  ```

  with:

  ```rust
      assert!(r.events.is_empty());
      assert_eq!(
          store.find_session("s1").unwrap().unwrap().open_turn_id.as_deref(),
          Some("t1")
      );
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&r.events), ["turn_ended_synthesized"]);
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
  }
  ```

  with:

  ```rust
      assert_eq!(kinds(&r.events), ["turn_ended_synthesized"]);
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
  }
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s3").unwrap().unwrap().lifecycle, "closed");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s3").unwrap().unwrap().lifecycle, "closed");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              "DROP INDEX events_by_kind;
  ```

  with:

  ```rust
              "DROP TRIGGER events_of_a_tombstone;
               DROP TRIGGER turns_of_a_tombstone;
               DROP TRIGGER pending_of_a_tombstone;
               DROP TRIGGER answers_of_a_tombstone;
               DROP TRIGGER catalog_of_a_tombstone;
               DROP TRIGGER a_tombstone_stays;
               DROP TABLE turn_attachments;
               DROP INDEX events_by_kind;
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(!store.session("s1").unwrap().unwrap().close_requested);
      assert!(store.session("s1").unwrap().unwrap().config.is_empty());
  ```

  with:

  ```rust
      assert!(!store.find_session("s1").unwrap().unwrap().close_requested);
      assert!(store.find_session("s1").unwrap().unwrap().config.is_empty());
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
      assert_eq!(
          store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
          Some("t1")
      );
  ```

  with:

  ```rust
      assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
      assert_eq!(
          store.find_session("s1").unwrap().unwrap().open_turn_id.as_deref(),
          Some("t1")
      );
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
  ```

  with:

  ```rust
          store.find_session("s1").unwrap().unwrap().open_turn_id.as_deref(),
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(created.is_empty());
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(s.open_turn_id.as_deref(), Some("t2"));
  ```

  with:

  ```rust
      assert!(created.is_empty());
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(s.open_turn_id.as_deref(), Some("t2"));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!((agent_session_id.as_str(), committed_seq), ("a1", 3));
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("starting", None));
  ```

  with:

  ```rust
      assert_eq!((agent_session_id.as_str(), committed_seq), ("a1", 3));
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("starting", None));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          .unwrap();
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
  ```

  with:

  ```rust
          .unwrap();
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      // Not applied: the session is active, not starting.
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
      store.close_now("s1").unwrap();
  ```

  with:

  ```rust
      // Not applied: the session is active, not starting.
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
      store.close_now("s1").unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          .unwrap();
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
          .unwrap();
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().failure_reason, None);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().failure_reason, None);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "failed");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "failed");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
      assert_eq!(store.session("s1").unwrap().unwrap().open_turn_id, None);

  ```

  with:

  ```rust
      assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
      assert_eq!(store.find_session("s1").unwrap().unwrap().open_turn_id, None);

  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s2").unwrap().unwrap().open_turn_id, None);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s2").unwrap().unwrap().open_turn_id, None);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().open_turn_id, None);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().open_turn_id, None);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(listed(&store, "s1"), ["session_started"]);
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
  }
  ```

  with:

  ```rust
      assert_eq!(listed(&store, "s1"), ["session_started"]);
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
  }
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let store = Store::open_in_memory().unwrap();
      store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
      store.reconcile_host("h1", &[]).unwrap();
      assert_eq!(
          store.session("s1").unwrap().unwrap().failure_reason.as_deref(),
          Some("start_not_delivered")
      );
      let created = store
          .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
  ```

  with:

  ```rust
      let store = Store::open_in_memory().unwrap();
      store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
      store.reconcile_host("h1", &[]).unwrap();
      assert_eq!(
          store.find_session("s1").unwrap().unwrap().failure_reason.as_deref(),
          Some("start_not_delivered")
      );
      let created = store
          .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&created), ["session_started"]);
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.failure_reason), ("active", None));
  ```

  with:

  ```rust
      assert_eq!(kinds(&created), ["session_started"]);
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s.lifecycle.as_str(), s.failure_reason), ("active", None));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          store.session("s1").unwrap().unwrap().failure_reason.as_deref(),
  ```

  with:

  ```rust
          store.find_session("s1").unwrap().unwrap().failure_reason.as_deref(),
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&created), ["start_failed"]);
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(kinds(&created), ["start_failed"]);
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(events[0].body["reason"], "host_offline");
      let s1 = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(events[0].body["reason"], "host_offline");
      let s1 = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s2").unwrap().unwrap().lifecycle, "active");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s2").unwrap().unwrap().lifecycle, "active");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&done.events), ["reattached"]);
      let s1 = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(kinds(&done.events), ["reattached"]);
      let s1 = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&done.events), ["host_restarted", "turn_ended_synthesized"]);
      let s1 = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(kinds(&done.events), ["host_restarted", "turn_ended_synthesized"]);
      let s1 = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&parked), ["session_parked"]);
      let s1 = store.session("s1").unwrap().unwrap();
      assert_eq!((s1.lifecycle.as_str(), s1.presumed_parked), ("parked", false));
  ```

  with:

  ```rust
      assert_eq!(kinds(&parked), ["session_parked"]);
      let s1 = store.find_session("s1").unwrap().unwrap();
      assert_eq!((s1.lifecycle.as_str(), s1.presumed_parked), ("parked", false));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(!store.session("s1").unwrap().unwrap().presumed_parked);

      store.presume_parked("h2").unwrap();
      store.close_now("s2").unwrap();
      let s2 = store.session("s2").unwrap().unwrap();
  ```

  with:

  ```rust
      assert!(!store.find_session("s1").unwrap().unwrap().presumed_parked);

      store.presume_parked("h2").unwrap();
      store.close_now("s2").unwrap();
      let s2 = store.find_session("s2").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let s1 = store.session("s1").unwrap().unwrap();
  ```

  with:

  ```rust
      let s1 = store.find_session("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&events), ["session_closed"]);
      let s2 = store.session("s2").unwrap().unwrap();
      assert_eq!((s2.lifecycle.as_str(), s2.presumed_parked), ("closed", false));
  ```

  with:

  ```rust
      assert_eq!(kinds(&events), ["session_closed"]);
      let s2 = store.find_session("s2").unwrap().unwrap();
      assert_eq!((s2.lifecycle.as_str(), s2.presumed_parked), ("closed", false));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "closed");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "closed");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "starting");
      assert_eq!(store.events("s1", 0, 100).unwrap(), before);
  ```

  with:

  ```rust
      assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
      assert_eq!(store.events("s1", 0, 100).unwrap(), before);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      started(&store);
      store.mark_failed_if_starting("s1", "not_attached").unwrap();
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(
          (s.lifecycle.as_str(), s.failure_reason.as_deref()),
  ```

  with:

  ```rust
      started(&store);
      store.mark_failed_if_starting("s1", "not_attached").unwrap();
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
          (s.lifecycle.as_str(), s.failure_reason.as_deref()),
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      store.mark_failed_if_starting("s1", "not_attached").unwrap();
      let s = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      store.mark_failed_if_starting("s1", "not_attached").unwrap();
      let s = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "starting");
      store
          .ingest("s1", 5, &SessionBody::session_started("r9", "a1"))
          .unwrap();
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
      store
          .ingest("s1", 5, &SessionBody::session_started("r9", "a1"))
          .unwrap();
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      store.session("s1").unwrap().unwrap().config
  ```

  with:

  ```rust
      store.find_session("s1").unwrap().unwrap().config
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      store.session("s1").unwrap().unwrap().activity
  ```

  with:

  ```rust
      store.find_session("s1").unwrap().unwrap().activity
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
  ```

  with:

  ```rust
          assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          let row = store.session(id).unwrap().unwrap();
  ```

  with:

  ```rust
          let row = store.find_session(id).unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let s2 = store.session("s2").unwrap().unwrap();
  ```

  with:

  ```rust
      let s2 = store.find_session("s2").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let s5 = store.session("s5").unwrap().unwrap();
      assert_eq!((s5.lifecycle.as_str(), s5.presumed_parked), ("closed", false));
      assert_eq!(store.session("s4").unwrap().unwrap().lifecycle, "active");
  ```

  with:

  ```rust
      let s5 = store.find_session("s5").unwrap().unwrap();
      assert_eq!((s5.lifecycle.as_str(), s5.presumed_parked), ("closed", false));
      assert_eq!(store.find_session("s4").unwrap().unwrap().lifecycle, "active");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().lifecycle, "active");
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(kinds(&events).contains(&"pending_cancelled"), "{:?}", kinds(&events));
      let row = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert!(kinds(&events).contains(&"pending_cancelled"), "{:?}", kinds(&events));
      let row = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let s = store.session("s1").unwrap().unwrap();
  ```

  with:

  ```rust
      let s = store.find_session("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      store.session("s1").unwrap().unwrap().title
  ```

  with:

  ```rust
      store.find_session("s1").unwrap().unwrap().title
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().config, before.current);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().config, before.current);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let row = store.session("s1").unwrap().unwrap();
      let item = store.session_item("s1").unwrap().unwrap();
  ```

  with:

  ```rust
      let row = store.find_session("s1").unwrap().unwrap();
      let item = store.find_session_item("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(store.session_item("nope").unwrap().is_none());
  ```

  with:

  ```rust
      assert!(store.find_session_item("nope").unwrap().is_none());
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session_item("s1").unwrap().unwrap().cwd, cwd);
  ```

  with:

  ```rust
      assert_eq!(store.find_session_item("s1").unwrap().unwrap().cwd, cwd);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(kinds(&created), ["git_state"]);
      let row = store.session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(kinds(&created), ["git_state"]);
      let row = store.find_session("s1").unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let item = store.session_item("s1").unwrap().unwrap();
      assert_eq!((item.git_branch.as_deref(), item.git_dirty), (Some("main"), Some(true)));

      let recency = store.session("s1").unwrap().unwrap().last_event_id;
  ```

  with:

  ```rust
      let item = store.find_session_item("s1").unwrap().unwrap();
      assert_eq!((item.git_branch.as_deref(), item.git_dirty), (Some("main"), Some(true)));

      let recency = store.find_session("s1").unwrap().unwrap().last_event_id;
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().last_event_id, recency);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().last_event_id, recency);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let row = store.session("s1").unwrap().unwrap();
      assert_eq!(row.base_commit.as_deref(), Some("c0ffee"));
      let item = store.session_item("s1").unwrap().unwrap();
  ```

  with:

  ```rust
      let row = store.find_session("s1").unwrap().unwrap();
      assert_eq!(row.base_commit.as_deref(), Some("c0ffee"));
      let item = store.find_session_item("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session_item("s1").unwrap().unwrap().git_branch, None);
  ```

  with:

  ```rust
      assert_eq!(store.find_session_item("s1").unwrap().unwrap().git_branch, None);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().git_worktree, Some(true));
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().git_worktree, Some(true));
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          store.session("s1").unwrap().unwrap().base_commit.as_deref(),
  ```

  with:

  ```rust
          store.find_session("s1").unwrap().unwrap().base_commit.as_deref(),
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().base_commit, None);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().base_commit, None);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let item = store.session_item("s1").unwrap().unwrap();
  ```

  with:

  ```rust
      let item = store.find_session_item("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let row = store.session("s1").unwrap().unwrap();
  ```

  with:

  ```rust
      let row = store.find_session("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      // migration (version 11) undone too.
      let conn = rusqlite::Connection::open(&db).unwrap();
      conn.execute_batch(
          "DROP INDEX events_by_kind;
  ```

  with:

  ```rust
      // migration (version 11) and plan 9a's (version 13) undone too.
      let conn = rusqlite::Connection::open(&db).unwrap();
      conn.execute_batch(
          "DROP TRIGGER events_of_a_tombstone;
           DROP TRIGGER turns_of_a_tombstone;
           DROP TRIGGER pending_of_a_tombstone;
           DROP TRIGGER answers_of_a_tombstone;
           DROP TRIGGER catalog_of_a_tombstone;
           DROP TRIGGER a_tombstone_stays;
           DROP TABLE turn_attachments;
           DROP INDEX attachments_by_hash;
           DROP INDEX events_by_kind;
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s-on-h1").unwrap().unwrap().hat_id, acme.id);
      assert_eq!(store.session("s-gone").unwrap().unwrap().hat_id, personal);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s-on-h1").unwrap().unwrap().hat_id, acme.id);
      assert_eq!(store.find_session("s-gone").unwrap().unwrap().hat_id, personal);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.session("s1").unwrap().unwrap().hat_id, acme.id);
  ```

  with:

  ```rust
      assert_eq!(store.find_session("s1").unwrap().unwrap().hat_id, acme.id);
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert_eq!(store.reassign_hat("s-nope", &acme.id).unwrap(), Reassign::NotFound);
  }
  ```

  with:

  ```rust
      assert_eq!(store.reassign_hat("s-nope", &acme.id).unwrap(), Reassign::NotFound);
  }

  // Plan 9a: session delete (ACP core §4.10). What is left of a deleted
  // session is its row, scrubbed, as a tombstone (decision 1), and one
  // `session_deleted` event; nothing writes to it again (decision 2, A1).

  use base64::Engine;
  use hennery_proto::rest::{AttachmentUsage, EventDto};
  use hennery_sessions::content;
  use hennery_sessions::store::{Deletion, Reassign, Unattached};
  use rusqlite::Connection;
  use std::path::{Path, PathBuf};

  /// A store over `hennery.db` in `dir`, and that path.
  fn file_store(dir: &Path) -> (Store, PathBuf) {
      let db = dir.join("hennery.db");
      (Store::open(&db).unwrap(), db)
  }

  /// Delete `id`, which is closed: its `session_deleted` event.
  fn delete(store: &Store, id: &str) -> EventDto {
      match store.delete_session(id, None).unwrap() {
          Deletion::Done {
              event,
              unconfirmed: false,
          } => event,
          other => panic!("not deleted: {other:?}"),
      }
  }

  /// `id` on `host`, started, closed and deleted: a tombstone.
  fn tombstone(store: &Store, id: &str, host: &str) {
      store.create_session(id, host, "fake", "/tmp", "hat-1", None).unwrap();
      store.ingest(id, 1, &SessionBody::session_started("r0", "a0")).unwrap();
      store.close_now(id).unwrap();
      delete(store, id);
  }

  /// Every column of a session's row, as stored.
  fn raw_row(conn: &Connection, id: &str) -> Vec<rusqlite::types::Value> {
      let mut stmt = conn.prepare("SELECT * FROM sessions WHERE id = ?1").unwrap();
      let width = stmt.column_count();
      stmt.query_row([id], |r| (0..width).map(|i| r.get(i)).collect())
          .unwrap()
  }

  fn event_kinds(conn: &Connection, id: &str) -> Vec<String> {
      let mut stmt = conn
          .prepare("SELECT kind FROM events WHERE session_id = ?1 ORDER BY event_id")
          .unwrap();
      stmt.query_map([id], |r| r.get(0))
          .unwrap()
          .map(Result::unwrap)
          .collect()
  }

  /// `len` bytes of a PNG, different for each `seed`.
  fn png(seed: u8, len: usize) -> Vec<u8> {
      let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
      bytes.extend((0..len - 8).map(|i| seed.wrapping_add(i as u8)));
      bytes
  }

  fn image(bytes: &[u8]) -> serde_json::Value {
      let data = base64::engine::general_purpose::STANDARD.encode(bytes);
      json!({ "type": "image", "mimeType": "image/png", "data": data })
  }

  fn sha(bytes: &[u8]) -> String {
      use sha2::{Digest, Sha256};
      hex::encode(Sha256::digest(bytes))
  }

  /// Check `content`, save its images and open `turn` of `session` with it,
  /// as the prompt route does.
  fn prompt_in(store: &Store, session: &str, turn: &str, content: Vec<serde_json::Value>) {
      let checked = content::check(content).unwrap();
      store.save_images(&checked.images).unwrap();
      assert!(store.open_prompt(session, turn, &checked).unwrap());
  }

  /// `id` on `h1`, active, in `cwd`.
  fn active(store: &Store, id: &str, cwd: &str) {
      store.create_session(id, "h1", "fake", cwd, "hat-1", None).unwrap();
      store.ingest(id, 1, &SessionBody::session_started("r0", "a0")).unwrap();
  }

  fn has_file(db: &Path, sha256: &str) -> bool {
      db.parent().unwrap().join("attachments").join(sha256).exists()
  }

  /// The tables that hold something of a session (a `session_id` column or
  /// a foreign key to `sessions`), plus the image tables, each with how many
  /// rows of it they hold: the session's own row, its events, turns and
  /// images by the ids and hashes it had.
  fn rows_of(conn: &Connection, id: &str, events: &[i64], turns: &[String], hashes: &[String]) -> Vec<(String, i64)> {
      let tables: Vec<String> = conn
          .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
          .unwrap()
          .query_map([], |r| r.get(0))
          .unwrap()
          .map(Result::unwrap)
          .collect();
      let list = |items: Vec<String>| items.join(", ");
      let quoted = |items: &[String]| list(items.iter().map(|s| format!("'{s}'")).collect());
      let mut out = Vec::new();
      for table in tables {
          let columns: Vec<String> = conn
              .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
              .unwrap()
              .query_map([], |r| r.get(0))
              .unwrap()
              .map(Result::unwrap)
              .collect();
          let refers: bool = conn
              .query_row(
                  &format!(
                      "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_list('{table}') WHERE \"table\" = 'sessions')"
                  ),
                  [],
                  |r| r.get(0),
              )
              .unwrap();
          let filter = match table.as_str() {
              "sessions" => format!("id = '{id}'"),
              "event_attachments" => format!("event_id IN ({})", list(events.iter().map(i64::to_string).collect())),
              "turn_attachments" => format!("turn_id IN ({})", quoted(turns)),
              "attachments" => format!("sha256 IN ({})", quoted(hashes)),
              _ if columns.iter().any(|c| c == "session_id") => format!("session_id = '{id}'"),
              _ => {
                  assert!(!refers, "{table} refers to sessions with no session_id: walk it here");
                  continue;
              }
          };
          let count: i64 = conn
              .query_row(&format!("SELECT count(*) FROM {table} WHERE {filter}"), [], |r| {
                  r.get(0)
              })
              .unwrap();
          out.push((table, count));
      }
      out
  }

  /// Decision 1: everything of the session goes but its row and one
  /// `session_deleted` event, walked over the schema, so a table added later
  /// with a `session_id` is walked too. Another session keeps all of its own.
  #[test]
  fn a_deleted_session_leaves_only_its_tombstone_row_and_one_event() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let (a, b, kept) = (png(1, 300), png(2, 300), png(3, 300));
      for id in ["s1", "s2"] {
          active(&store, id, "/srv/app");
          store.ingest(id, 2, &titled("a title")).unwrap();
          store.ingest(id, 3, &commands(&["build"])).unwrap();
          store.ingest(id, 4, &git(Some("main"), true, Some("c0ffee"))).unwrap();
      }
      // s1: a started turn with an image (its event links it), a question
      // with an answer queued; then a second turn that never started, with
      // another image (only the turn links it).
      prompt_in(
          &store,
          "s1",
          "t1",
          vec![json!({"type": "text", "text": "see"}), image(&a)],
      );
      store.ingest("s1", 5, &turn_started("t1")).unwrap();
      store.ingest("s1", 6, &permission("p1")).unwrap();
      assert!(matches!(
          store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
          AnswerSubmission::Queued(_)
      ));
      store.ingest("s1", 7, &ended("t1")).unwrap();
      prompt_in(&store, "s1", "t2", vec![image(&b)]);
      prompt_in(&store, "s2", "t9", vec![image(&kept)]);
      store.ingest("s2", 5, &turn_started("t9")).unwrap();
      store.close_now("s1").unwrap();

      let conn = Connection::open(&db).unwrap();
      let ids = |sql: &str, id: &str| -> Vec<String> {
          conn.prepare(sql)
              .unwrap()
              .query_map([id], |r| r.get(0))
              .unwrap()
              .map(Result::unwrap)
              .collect()
      };
      let events: Vec<i64> = conn
          .prepare("SELECT event_id FROM events WHERE session_id = 's1'")
          .unwrap()
          .query_map([], |r| r.get(0))
          .unwrap()
          .map(Result::unwrap)
          .collect();
      let turns = ids("SELECT turn_id FROM turns WHERE session_id = ?1", "s1");
      let hashes = vec![sha(&a), sha(&b)];
      let before = rows_of(&conn, "s1", &events, &turns, &hashes);
      let walked: Vec<&str> = before.iter().map(|(t, _)| t.as_str()).collect();
      for table in [
          "answer_queue",
          "attachments",
          "event_attachments",
          "events",
          "pending",
          "session_catalog",
          "sessions",
          "turn_attachments",
          "turns",
      ] {
          assert!(walked.contains(&table), "{table} not walked: {walked:?}");
      }
      assert!(
          before.iter().all(|(_, n)| *n > 0),
          "nothing to delete somewhere: {before:?}"
      );
      let s2_before = {
          let s2_events: Vec<i64> = conn
              .prepare("SELECT event_id FROM events WHERE session_id = 's2'")
              .unwrap()
              .query_map([], |r| r.get(0))
              .unwrap()
              .map(Result::unwrap)
              .collect();
          let s2_turns = ids("SELECT turn_id FROM turns WHERE session_id = ?1", "s2");
          (
              s2_events.clone(),
              s2_turns.clone(),
              rows_of(&conn, "s2", &s2_events, &s2_turns, &[sha(&kept)]),
          )
      };

      let event = delete(&store, "s1");
      assert_eq!((event.kind.as_str(), &event.body), ("session_deleted", &json!({})));
      let after = rows_of(&conn, "s1", &events, &turns, &hashes);
      for (table, n) in &after {
          let left = match table.as_str() {
              "sessions" | "events" => 1,
              _ => 0,
          };
          assert_eq!(*n, left, "{table}: {after:?}");
      }
      assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"]);
      assert!(!has_file(&db, &sha(&a)) && !has_file(&db, &sha(&b)));
      let (s2_events, s2_turns, s2_rows) = s2_before;
      assert_eq!(rows_of(&conn, "s2", &s2_events, &s2_turns, &[sha(&kept)]), s2_rows);
      assert!(has_file(&db, &sha(&kept)));
      // A tombstone is no session: deleted again, it is not found.
      assert!(matches!(store.delete_session("s1", None).unwrap(), Deletion::NotFound));
      assert!(matches!(
          store.delete_session("s-nope", None).unwrap(),
          Deletion::NotFound
      ));
  }

  /// Decision 1: the tombstone keeps its id, owner, host, hat, creation and
  /// recency (its `session_deleted` event); every column that could hold
  /// client data is cleared. Decision 3: no accessor and no list finds it.
  #[test]
  fn a_tombstone_is_scrubbed_and_found_by_no_accessor_or_list() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      store
          .create_session("s1", "h1", "fake", "/srv/app", "hat-1", Some("rule-1"))
          .unwrap();
      store
          .ingest(
              "s1",
              1,
              &SessionBody::SessionStarted {
                  request_id: "r0".into(),
                  agent_session_id: "agent-1".into(),
                  indexed: catalogue("opus", "plan"),
              },
          )
          .unwrap();
      store.ingest("s1", 2, &titled("a title")).unwrap();
      store.ingest("s1", 3, &git(Some("main"), true, Some("c0ffee"))).unwrap();
      store.open_turn("s1", "t1", &prompt_text()).unwrap();
      store.presume_parked("h1").unwrap();
      // Every column that can hold client data holds some.
      let conn = Connection::open(&db).unwrap();
      conn.execute("UPDATE sessions SET failure_reason = 'x' WHERE id = 's1'", [])
          .unwrap();
      let created_at: String = conn
          .query_row("SELECT created_at FROM sessions WHERE id = 's1'", [], |r| r.get(0))
          .unwrap();
      let Deletion::Done { event, unconfirmed } = store
          .delete_session(
              "s1",
              Some(&Unattached {
                  lifecycle: "parked".into(),
                  presumed_parked: true,
              }),
          )
          .unwrap()
      else {
          panic!("not deleted");
      };
      assert!(unconfirmed);

      type Scrubbed = (
          (String, String, String, String, String, String),
          (
              Option<String>,
              Option<String>,
              Option<String>,
              Option<bool>,
              Option<bool>,
              Option<String>,
          ),
          (
              Option<String>,
              Option<String>,
              Option<String>,
              Option<String>,
              Option<String>,
              Option<String>,
          ),
          (Option<String>, bool, bool),
          (String, Option<i64>, bool),
      );
      let row: Scrubbed = conn
          .query_row(
              "SELECT host_id, hat_id, agent, cwd, lifecycle, created_at,
                      title, git_branch, base_commit, git_dirty, git_worktree, model,
                      mode, config_axes, agent_session_id, failure_reason, hat_rule_id, open_turn_id,
                      activity, presumed_parked, close_requested,
                      last_event_at, last_event_id, owner_id = ?1
               FROM sessions WHERE id = 's1'",
              [store.owner_id()],
              |r| {
                  Ok((
                      (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?),
                      (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?),
                      (r.get(12)?, r.get(13)?, r.get(14)?, r.get(15)?, r.get(16)?, r.get(17)?),
                      (r.get(18)?, r.get(19)?, r.get(20)?),
                      (r.get(21)?, r.get(22)?, r.get(23)?),
                  ))
              },
          )
          .unwrap();
      assert_eq!(
          row,
          (
              (
                  "h1".into(),
                  "hat-1".into(),
                  String::new(),
                  String::new(),
                  "deleted".into(),
                  created_at
              ),
              (None, None, None, None, None, None),
              (None, None, None, None, None, None),
              (None, false, false),
              (event.ts.clone(), Some(event.event_id), true),
          )
      );

      // The columns no closed session holds set are cleared too.
      store
          .create_session("s2", "h1", "fake", "/srv/b", "hat-1", None)
          .unwrap();
      store.close_now("s2").unwrap();
      conn.execute(
          "UPDATE sessions SET open_turn_id = 't9', activity = 'idle', presumed_parked = 1, close_requested = 1
           WHERE id = 's2'",
          [],
      )
      .unwrap();
      delete(&store, "s2");
      let flags: (Option<String>, Option<String>, bool, bool) = conn
          .query_row(
              "SELECT open_turn_id, activity, presumed_parked, close_requested FROM sessions WHERE id = 's2'",
              [],
              |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
          )
          .unwrap();
      assert_eq!(flags, (None, None, false, false));

      assert_eq!(store.find_session("s1").unwrap(), None);
      assert_eq!(store.find_session_item("s1").unwrap(), None);
      for query in [
          ListQuery::default(),
          ListQuery {
              search: Some("s1"),
              ..ListQuery::default()
          },
          ListQuery {
              hat: Some("hat-1"),
              ..ListQuery::default()
          },
      ] {
          assert!(store.list(&query).unwrap().sessions.is_empty(), "{query:?}");
      }
  }

  /// A8: once deleted, a session's title and cwd are in neither the database
  /// file nor its WAL: `secure_delete` zeroes what is deleted, and a
  /// checkpoint folds the WAL back.
  #[test]
  fn a_deleted_sessions_title_and_cwd_are_not_left_in_the_database_files() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let (title, cwd) = ("zq-title-91c2be", "/srv/zq-cwd-7f3e0a");
      active(&store, "s1", cwd);
      store.ingest("s1", 2, &titled(title)).unwrap();
      store.ingest("s1", 3, &git(Some("main"), false, None)).unwrap();
      store.close_now("s1").unwrap();
      let files = || -> Vec<u8> {
          let mut bytes = std::fs::read(&db).unwrap();
          bytes.extend(std::fs::read(db.with_extension("db-wal")).unwrap_or_default());
          bytes
      };
      let holds = |bytes: &[u8], needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
      let before = files();
      assert!(
          holds(&before, title) && holds(&before, cwd),
          "not written to begin with"
      );

      delete(&store, "s1");
      // The delete's own checkpoint folded the WAL back already.
      let now = files();
      assert!(!holds(&now, title) && !holds(&now, cwd), "left in the WAL");
      drop(store);
      let conn = Connection::open(&db).unwrap();
      conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
          .unwrap();
      drop(conn);
      let after = files();
      assert!(!holds(&after, title), "the title is still in the files");
      assert!(!holds(&after, cwd), "the cwd is still in the files");
  }

  /// Decision 6: an image is the owner's while a turn or an event of a kept
  /// session shows it; its row and its file go with the last of them, and
  /// the usage drops by what went.
  #[test]
  fn an_image_two_sessions_show_stays_until_the_second_is_deleted() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let (shared, own) = (png(1, 1000), png(2, 500));
      active(&store, "s1", "/srv/a");
      active(&store, "s2", "/srv/b");
      prompt_in(&store, "s1", "t1", vec![image(&shared), image(&own)]);
      store.ingest("s1", 2, &turn_started("t1")).unwrap();
      prompt_in(&store, "s2", "t2", vec![image(&shared)]);
      store.ingest("s2", 2, &turn_started("t2")).unwrap();
      assert_eq!(
          store.attachment_usage().unwrap(),
          AttachmentUsage { count: 2, bytes: 1500 }
      );
      for id in ["s1", "s2"] {
          store.close_now(id).unwrap();
      }
      // s2 shows the shared image by its event alone: its turn's link gone,
      // as for a turn of a database from before the links were kept.
      let conn = Connection::open(&db).unwrap();
      conn.execute("DELETE FROM turn_attachments WHERE turn_id = 't2'", [])
          .unwrap();

      delete(&store, "s1");
      assert_eq!(
          store.attachment_usage().unwrap(),
          AttachmentUsage { count: 1, bytes: 1000 }
      );
      assert!(store.attachment(&sha(&shared)).unwrap().is_some());
      assert!(store.attachment(&sha(&own)).unwrap().is_none());
      assert!(has_file(&db, &sha(&shared)) && !has_file(&db, &sha(&own)));

      delete(&store, "s2");
      assert_eq!(
          store.attachment_usage().unwrap(),
          AttachmentUsage { count: 0, bytes: 0 }
      );
      assert!(!has_file(&db, &sha(&shared)));
  }

  /// Decision 6: a turn that never started (no `user_turn` event) still
  /// shows its image, so another session's delete keeps it.
  #[test]
  fn an_image_only_a_turn_shows_is_still_referenced() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let img = png(7, 400);
      active(&store, "s1", "/srv/a");
      active(&store, "s2", "/srv/b");
      prompt_in(&store, "s1", "t1", vec![image(&img)]);
      store.ingest("s1", 2, &turn_started("t1")).unwrap();
      // s2's turn is sent, never started.
      prompt_in(&store, "s2", "t2", vec![image(&img)]);
      store.close_now("s1").unwrap();
      delete(&store, "s1");
      assert_eq!(store.attachment_usage().unwrap().count, 1);
      assert!(has_file(&db, &sha(&img)));
  }

  /// Decision 6: an abandoned turn's images go with it, unless something
  /// else still shows them.
  #[test]
  fn abandoning_a_turn_removes_an_image_only_it_showed() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let (only, shared) = (png(1, 300), png(2, 200));
      active(&store, "s1", "/srv/a");
      active(&store, "s2", "/srv/b");
      prompt_in(&store, "s2", "t2", vec![image(&shared)]);
      prompt_in(&store, "s1", "t1", vec![image(&only), image(&shared)]);
      store.abandon_turn("s1", "t1").unwrap();
      assert_eq!(
          store.attachment_usage().unwrap(),
          AttachmentUsage { count: 1, bytes: 200 }
      );
      assert!(!has_file(&db, &sha(&only)) && has_file(&db, &sha(&shared)));
      assert_eq!(store.turn_state("t1").unwrap(), None);
  }

  /// Decision 6: a prompt whose image file went missing since it was saved
  /// (a delete in between) writes it again as it opens the turn, and links
  /// the turn to each image at its block's index.
  #[test]
  fn opening_a_prompt_rewrites_a_missing_image_and_links_the_turn() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let img = png(5, 300);
      active(&store, "s1", "/srv/a");
      let checked = content::check(vec![json!({"type": "text", "text": "x"}), image(&img), image(&img)]).unwrap();
      store.save_images(&checked.images).unwrap();
      std::fs::remove_file(db.parent().unwrap().join("attachments").join(sha(&img))).unwrap();
      assert!(store.open_prompt("s1", "t1", &checked).unwrap());
      assert!(has_file(&db, &sha(&img)));
      assert_eq!(store.attachment(&sha(&img)).unwrap().unwrap().bytes, img);
      let conn = Connection::open(&db).unwrap();
      let links: Vec<(String, i64)> = conn
          .prepare("SELECT sha256, position FROM turn_attachments WHERE turn_id = 't1' ORDER BY position")
          .unwrap()
          .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
          .unwrap()
          .map(Result::unwrap)
          .collect();
      assert_eq!(links, [(sha(&img), 1), (sha(&img), 2)]);
  }

  /// Decision 5, A4: only a closed session is deleted as it is. One the
  /// route judged to have no adapter it can reach is closed first, if it is
  /// still exactly what the route saw (compare-and-set); else refused, and
  /// nothing changes.
  #[test]
  fn a_session_not_closed_is_deleted_only_as_the_route_judged_it() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let conn = Connection::open(&db).unwrap();
      let judged = |lifecycle: &str, presumed_parked: bool| Unattached {
          lifecycle: lifecycle.into(),
          presumed_parked,
      };
      let refused = |id: &str, unattached: Option<&Unattached>, lifecycle: &str| {
          let row = raw_row(&conn, id);
          let kinds = event_kinds(&conn, id);
          match store.delete_session(id, unattached).unwrap() {
              Deletion::Refused(found) => assert_eq!(found, lifecycle),
              other => panic!("not refused: {other:?}"),
          }
          assert_eq!((raw_row(&conn, id), event_kinds(&conn, id)), (row, kinds));
      };
      // `unconfirmed`: its host may still run it (A13).
      let deleted =
          |id: &str, unattached: &Unattached, may_run: bool| match store.delete_session(id, Some(unattached)).unwrap() {
              Deletion::Done { event, unconfirmed } => {
                  assert_eq!(unconfirmed, may_run, "{id}");
                  assert_eq!(event.kind, "session_deleted");
                  assert_eq!(event_kinds(&conn, id), ["session_deleted"]);
              }
              other => panic!("not deleted: {other:?}"),
          };

      store
          .create_session("s1", "h1", "fake", "/srv/a", "hat-1", None)
          .unwrap();
      refused("s1", None, "starting");
      refused("s1", Some(&judged("active", false)), "starting");
      deleted("s1", &judged("starting", false), true);

      active(&store, "s2", "/srv/b");
      store.open_turn("s2", "t2", &prompt_text()).unwrap();
      refused("s2", None, "active");
      refused("s2", Some(&judged("active", true)), "active");
      store.presume_parked("h1").unwrap();
      // The route saw it active on an offline host; it is presumed parked by now.
      refused("s2", Some(&judged("active", false)), "parked");
      deleted("s2", &judged("parked", true), true);

      active(&store, "s3", "/srv/c");
      store
          .ingest(
              "s3",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      refused("s3", None, "parked");
      refused("s3", Some(&judged("parked", true)), "parked");
      deleted("s3", &judged("parked", false), false);

      active(&store, "s4", "/srv/d");
      refused("s4", Some(&judged("active", true)), "active");
      deleted("s4", &judged("active", false), true);

      store
          .create_session("s5", "h1", "fake", "/srv/e", "hat-1", None)
          .unwrap();
      store.mark_failed("s5", "spawn").unwrap();
      refused("s5", None, "failed");
      deleted("s5", &judged("failed", false), false);
  }

  /// Decision 2, A9: the schema itself refuses anything new for a tombstone,
  /// whoever writes it.
  #[test]
  fn the_schema_refuses_writes_for_a_tombstone() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      tombstone(&store, "s1", "h1");
      let conn = Connection::open(&db).unwrap();
      conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
      let owner = store.owner_id();
      for sql in [
          "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id) VALUES ('s1', 9, 'x', '{}', 't', ?1)",
          "INSERT INTO events(session_id, host_seq, kind, body, ts, owner_id)
           VALUES ('s1', NULL, 'session_deleted', '{}', 't', ?1)",
          "INSERT INTO turns(turn_id, session_id, content, created_at, owner_id) VALUES ('t9', 's1', '[]', 't', ?1)",
          "INSERT INTO pending(pending_id, session_id, kind, payload, state, opened_at, owner_id)
           VALUES ('p9', 's1', 'permission', '{}', 'open', 't', ?1)",
          "INSERT INTO answer_queue(pending_id, session_id, request_id, answer, submitted_at, owner_id)
           VALUES ('p9', 's1', 'r9', '{}', 't', ?1)",
          "INSERT INTO session_catalog(session_id, config_options, updated_at, owner_id) VALUES ('s1', '[]', 't', ?1)",
          "UPDATE sessions SET title = 'back' WHERE id = 's1' AND ?1 = ?1",
          "UPDATE sessions SET lifecycle = 'closed' WHERE id = 's1' AND ?1 = ?1",
      ] {
          let err = conn.execute(sql, [owner]).unwrap_err().to_string();
          assert!(err.contains("a deleted session"), "{sql}: {err}");
      }
      assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"]);
  }

  /// A1, decision 2: the store's own writers leave a tombstone alone and
  /// answer as they would for a session with nothing to do; one that would
  /// write an event for it fails as for an unknown session.
  #[test]
  fn the_stores_writers_leave_a_tombstone_alone() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      tombstone(&store, "s1", "h1");
      let conn = Connection::open(&db).unwrap();
      let row = raw_row(&conn, "s1");

      assert!(store.ingest("s1", 2, &update(1)).unwrap().is_empty());
      assert!(
          store
              .ingest("s1", 1, &SessionBody::session_started("r0", "a0"))
              .unwrap()
              .is_empty()
      );
      assert!(store.ingest("s1", 3, &SessionBody::SessionClosed).unwrap().is_empty());
      assert!(store.close_now("s1").unwrap().is_empty());
      assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
      store.mark_failed("s1", "late").unwrap();
      store.mark_failed_if_starting("s1", "late").unwrap();
      assert!(!store.open_turn("s1", "t1", &prompt_text()).unwrap());
      for err in [
          store.record_park_request("s1").unwrap_err().to_string(),
          store.record_close_request("s1").unwrap_err().to_string(),
      ] {
          assert!(err.contains("no session s1"), "{err}");
      }
      assert_eq!(store.request_resume("s1", "hat-1").unwrap(), ResumeRequest::NotFound);
      assert_eq!(store.reassign_hat("s1", "hat-1").unwrap(), Reassign::NotFound);
      assert_eq!(store.catalog("s1").unwrap(), None);
      assert!(matches!(
          store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
          AnswerSubmission::NotFound
      ));
      // Its host's frames still find it, to ack and discard them (decision 4).
      assert_eq!(store.session_host("s1").unwrap().as_deref(), Some("h1"));
      assert_eq!(store.session_host("s-nope").unwrap(), None);
      assert_eq!(raw_row(&conn, "s1"), row);
      assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"]);
  }

  /// A1: a tombstone, listed by its host as attached or not, goes through
  /// every statement over a host's sessions untouched, and is no reason to
  /// think its host runs anything.
  #[test]
  fn a_tombstone_goes_through_the_bulk_functions_untouched() {
      for listed in [false, true] {
          let dir = tempfile::tempdir().unwrap();
          let (store, db) = file_store(dir.path());
          tombstone(&store, "s1", "h1");
          active(&store, "s2", "/srv/b");
          let conn = Connection::open(&db).unwrap();
          let row = raw_row(&conn, "s1");
          let attached: Vec<AttachedSession> = if listed {
              vec![attached("s1", None), attached("s2", None)]
          } else {
              vec![attached("s2", None)]
          };
          assert_eq!(store.hosts_with_active_sessions().unwrap(), ["h1"]);
          store.presume_parked("h1").unwrap();
          // Decision 4: listed, its host is told to close its adapter; its
          // answer, either way, changes nothing.
          let reconciled = store.reconcile_host("h1", &attached).unwrap();
          assert!(reconciled.events.iter().all(|e| e.session_id != "s1"), "{reconciled:?}");
          assert_eq!(reconciled.close.contains(&"s1".to_string()), listed, "{reconciled:?}");
          assert!(store.ingest("s1", 2, &SessionBody::SessionClosed).unwrap().is_empty());
          assert!(store.close_after_rejected_reconcile_close("s1").unwrap().is_empty());
          store.revoke_host("h1").unwrap();
          assert!(store.hosts_with_active_sessions().unwrap().is_empty());
          let again = store.reconcile_host("h1", &attached).unwrap();
          assert_eq!(again.close.contains(&"s1".to_string()), listed, "{again:?}");
          assert_eq!(raw_row(&conn, "s1"), row, "listed: {listed}");
          assert_eq!(event_kinds(&conn, "s1"), ["session_deleted"], "listed: {listed}");
      }
  }

  /// R1–R4: a delete removes its project recent (host, hat, cwd: exact
  /// bytes) unless another session kept of that host and hat has that cwd.
  #[test]
  fn a_delete_removes_its_recent_unless_another_kept_session_has_that_cwd() {
      use hennery_kernel::hosts::{Enrollment, Hosts};
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let hosts = Hosts::open(&db).unwrap();
      let mut hat = String::new();
      for (host, key) in [
          ("h1", "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"),
          ("h2", "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"),
      ] {
          let enrollment = Enrollment {
              public_key: key.into(),
              name: "test".into(),
              host_version: "0".into(),
              platform: "test".into(),
          };
          hosts.register(host, &enrollment, 1_800_000_000).unwrap();
          hat = hosts.host(host).unwrap().unwrap().default_hat_id;
      }
      for (id, host, cwd) in [
          ("s1", "h1", "/p/a"),
          ("s2", "h1", "/p/a"),
          ("s3", "h1", "/p/b"),
          ("s4", "h2", "/p/b"),
          ("s5", "h1", "/p/A"),
      ] {
          store.create_session(id, host, "fake", cwd, &hat, None).unwrap();
          store.close_now(id).unwrap();
          assert!(hosts.remember(host, &hat, cwd, 1_800_000_000).unwrap());
      }
      let conn = Connection::open(&db).unwrap();
      let recents = || -> Vec<(String, String)> {
          conn.prepare("SELECT host_id, path FROM project_recents ORDER BY host_id, path")
              .unwrap()
              .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
              .unwrap()
              .map(Result::unwrap)
              .collect()
      };
      let recent = |host: &str, path: &str| (host.to_string(), path.to_string());
      delete(&store, "s1");
      assert!(recents().contains(&recent("h1", "/p/a")), "s2 still has it");
      // h2's s4 is another host's: h1's recent goes.
      delete(&store, "s3");
      assert_eq!(
          recents(),
          [recent("h1", "/p/A"), recent("h1", "/p/a"), recent("h2", "/p/b")]
      );
      // s5's `/p/A` is not `/p/a`, and s1's tombstone keeps nothing.
      delete(&store, "s2");
      assert_eq!(recents(), [recent("h1", "/p/A"), recent("h2", "/p/b")]);
  }

  /// Decision 6, A6: files are shared by hash, so the owner's row goes with
  /// the owner's last reference, but the file stays while another owner's
  /// row names it.
  #[test]
  fn an_image_file_another_owner_holds_stays_when_the_owners_row_goes() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let img = png(9, 300);
      active(&store, "s1", "/srv/a");
      prompt_in(&store, "s1", "t1", vec![image(&img)]);
      let conn = Connection::open(&db).unwrap();
      conn.execute(
          "INSERT INTO owners(id, created_at, set_up_at)
           VALUES ('owner-00000000000000b2', 9223372036854775807, 9223372036854775807)",
          [],
      )
      .unwrap();
      conn.execute(
          "INSERT INTO attachments(owner_id, sha256, mime, size, created_at)
           VALUES ('owner-00000000000000b2', ?1, 'image/png', 300, 't')",
          [sha(&img)],
      )
      .unwrap();
      store.close_now("s1").unwrap();
      delete(&store, "s1");
      assert_eq!(store.attachment_usage().unwrap().count, 0);
      assert!(has_file(&db, &sha(&img)), "another owner's image lost its file");
  }
  ```

  In `crates/hennery-testkit/tests/e2e.rs`, replace:

  ```rust
      let row = collector.state.store.session(session_id).unwrap().unwrap();
  ```

  with:

  ```rust
      let row = collector.state.store.find_session(session_id).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/e2e.rs`, replace:

  ```rust
          let row = collector.state.store.session(session).unwrap().unwrap();
  ```

  with:

  ```rust
          let row = collector.state.store.find_session(session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/e2e.rs`, replace:

  ```rust
          collector.state.store.session(&session).unwrap().unwrap().lifecycle,
  ```

  with:

  ```rust
          collector.state.store.find_session(&session).unwrap().unwrap().lifecycle,
  ```

  In `crates/hennery-testkit/tests/e2e.rs`, replace:

  ```rust
          let row = away.state.store.session(&session).unwrap().unwrap();
  ```

  with:

  ```rust
          let row = away.state.store.find_session(&session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/e2e.rs`, replace:

  ```rust
      let row = collector.state.store.session(&session).unwrap().unwrap();
  ```

  with:

  ```rust
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
      );
      host.nothing_more().await;
      assert_eq!(
          collector.state.store.session(&session).unwrap().unwrap().open_turn_id,
          None
      );
      assert!(collector.files().is_empty(), "{:?}", collector.files());
  ```

  with:

  ```rust
      );
      host.nothing_more().await;
      assert_eq!(
          collector
              .state
              .store
              .find_session(&session)
              .unwrap()
              .unwrap()
              .open_turn_id,
          None
      );
      assert!(collector.files().is_empty(), "{:?}", collector.files());
  ```

  In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
      host.nothing_more().await;
      assert_eq!(
          collector.state.store.session(&session).unwrap().unwrap().open_turn_id,
          None
      );
  ```

  with:

  ```rust
      host.nothing_more().await;
      assert_eq!(
          collector
              .state
              .store
              .find_session(&session)
              .unwrap()
              .unwrap()
              .open_turn_id,
          None
      );
  ```

  In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
          collector.state.store.session(&session).unwrap().unwrap().open_turn_id,
  ```

  with:

  ```rust
          collector
              .state
              .store
              .find_session(&session)
              .unwrap()
              .unwrap()
              .open_turn_id,
  ```

  In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          87,
  ```

  with:

  ```rust
          105,
  ```

  In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          "`kernel_owner`'s query finds the owner; the rest is the migrations' bookkeeping and their unit tests",
      ),
  ```

  with:

  ```rust
          "`kernel_owner`'s query finds the owner; the rest is the migrations' bookkeeping and their unit tests",
      ),
      (
          "hennery-sessions/src/shared_files.rs",
          "attachment files are shared by hash across owners: whether any owner's row still names one, before its file \
           is removed (plan 9a A6); an existence read, no data, no write",
      ),
  ```

  In `crates/hennery-testkit/tests/projects.rs`, replace:

  ```rust
          .session(&start.session_id)
  ```

  with:

  ```rust
          .find_session(&start.session_id)
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          self.state.store.session(session).unwrap().unwrap().lifecycle
  ```

  with:

  ```rust
          self.state.store.find_session(session).unwrap().unwrap().lifecycle
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let _host = ScriptedHost::connect(&collector, vec![], 0).await;
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      let _host = ScriptedHost::connect(&collector, vec![], 0).await;
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      collector.state.store.session(session).unwrap().unwrap().presumed_parked
  ```

  with:

  ```rust
      collector
          .state
          .store
          .find_session(session)
          .unwrap()
          .unwrap()
          .presumed_parked
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      assert_eq!((status, body["code"].as_str()), (502, Some("agent_has_no_record")));
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!((status, body["code"].as_str()), (502, Some("agent_has_no_record")));
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          assert_eq!((status, body["code"].as_str()), (502, Some(code)), "{body}");
          let row = collector.state.store.session(&session).unwrap().unwrap();
          assert_eq!(
  ```

  with:

  ```rust
          assert_eq!((status, body["code"].as_str()), (502, Some(code)), "{body}");
          let row = collector.state.store.find_session(&session).unwrap().unwrap();
          assert_eq!(
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let _host = ScriptedHost::connect(&collector, vec![], seq).await;
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      let _host = ScriptedHost::connect(&collector, vec![], seq).await;
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let item = collector.state.store.session_item(&session).unwrap().unwrap();
  ```

  with:

  ```rust
      let item = collector.state.store.find_session_item(&session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let row = collector.state.store.session(session).unwrap().unwrap();
  ```

  with:

  ```rust
      let row = collector.state.store.find_session(session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      wait_for("turn slot free", || async {
          let row = collector.state.store.session(&session).unwrap().unwrap();
          row.open_turn_id.is_none().then_some(())
  ```

  with:

  ```rust
      wait_for("turn slot free", || async {
          let row = collector.state.store.find_session(&session).unwrap().unwrap();
          row.open_turn_id.is_none().then_some(())
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      assert_eq!(body, json!({ "turn_id": turn, "outcome": "cancelled" }));
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!((row.open_turn_id, row.activity.as_deref()), (None, Some("idle")));
  ```

  with:

  ```rust
      assert_eq!(body, json!({ "turn_id": turn, "outcome": "cancelled" }));
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!((row.open_turn_id, row.activity.as_deref()), (None, Some("idle")));
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      host.closed().await;

      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!(
          (row.lifecycle.as_str(), row.presumed_parked, row.open_turn_id),
  ```

  with:

  ```rust
      host.closed().await;

      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!(
          (row.lifecycle.as_str(), row.presumed_parked, row.open_turn_id),
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          .await;
      tokio::time::sleep(Duration::from_millis(200)).await;
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
      assert!(!collector.event_kinds(&session).contains(&"reattached".to_string()));
  ```

  with:

  ```rust
          .await;
      tokio::time::sleep(Duration::from_millis(200)).await;
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
      assert!(!collector.event_kinds(&session).contains(&"reattached".to_string()));
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      host.closed().await;
      wait_for("the session parked", || async {
          let row = collector.state.store.session(&session).unwrap().unwrap();
          (row.lifecycle == "parked" && row.presumed_parked).then_some(())
      })
  ```

  with:

  ```rust
      host.closed().await;
      wait_for("the session parked", || async {
          let row = collector.state.store.find_session(&session).unwrap().unwrap();
          (row.lifecycle == "parked" && row.presumed_parked).then_some(())
      })
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          let row = collector.state.store.session(&session).unwrap().unwrap();
  ```

  with:

  ```rust
          let row = collector.state.store.find_session(&session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      tokio::time::sleep(Duration::from_millis(200)).await;
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
  ```

  with:

  ```rust
      tokio::time::sleep(Duration::from_millis(200)).await;
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust

      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust

      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              .session(&session)
  ```

  with:

  ```rust
              .find_session(&session)
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let row = collector.state.store.session(&session).unwrap().unwrap();
  ```

  with:

  ```rust
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
  ```

  In `crates/hennery-testkit/tests/resolve.rs`, replace:

  ```rust
      assert_eq!(status, 202, "{body}");
      let row = collector.state.store.session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  with:

  ```rust
      assert_eq!(status, 202, "{body}");
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      assert_eq!(
  ```

  In `crates/hennery-testkit/tests/resolve.rs`, replace:

  ```rust
      assert_eq!(call.await.unwrap().0, 202);
      let row = collector.state.store.session(&session).unwrap().unwrap();
      let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
  ```

  with:

  ```rust
      assert_eq!(call.await.unwrap().0, 202);
      let row = collector.state.store.find_session(&session).unwrap().unwrap();
      let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
  ```

  In `crates/hennery-testkit/tests/resolve.rs`, replace:

  ```rust
      assert_eq!(call.await.unwrap().0, 202);
      host.parked(&session).await;
      wait_for("parked", || async {
          let row = collector.state.store.session(&session).unwrap().unwrap();
          (row.lifecycle == "parked").then_some(())
      })
      .await;
  ```

  with:

  ```rust
      assert_eq!(call.await.unwrap().0, 202);
      host.parked(&session).await;
      wait_for("parked", || async {
          let row = collector.state.store.find_session(&session).unwrap().unwrap();
          (row.lifecycle == "parked").then_some(())
      })
      .await;
  ```

  In `crates/hennery-testkit/tests/resolve.rs`, replace:

  ```rust
          collector.state.store.session(&session).unwrap().unwrap().lifecycle,
  ```

  with:

  ```rust
          collector.state.store.find_session(&session).unwrap().unwrap().lifecycle,
  ```

  In `crates/hennery-testkit/tests/resolve.rs`, replace:

  ```rust
          let row = collector.state.store.session(&session).unwrap().unwrap();
  ```

  with:

  ```rust
          let row = collector.state.store.find_session(&session).unwrap().unwrap();
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-sessions --locked --test store`
Expected: FAIL to compile: `no method named find_session found for struct Store`, `unresolved import hennery_sessions::store::Deletion`.

- [ ] **Step 3: The store**

  In `crates/hennery-kernel/src/db.rs`, replace:

  ```rust
      conn.pragma_update(None, "foreign_keys", "ON")?;
      conn.busy_timeout(std::time::Duration::from_secs(5))?;
  ```

  with:

  ```rust
      conn.pragma_update(None, "foreign_keys", "ON")?;
      // What is deleted is overwritten with zeros, not left in free space
      // for anyone who reads the file (plan 9a A8): a deleted session's
      // title, cwd and timeline must not outlive the delete in the database.
      // The WAL keeps old pages until a checkpoint, which a delete runs.
      conn.pragma_update(None, "secure_delete", "ON")?;
      conn.busy_timeout(std::time::Duration::from_secs(5))?;
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      let (session, item) = match (state.store.session(&id), state.store.session_item(&id)) {
  ```

  with:

  ```rust
      let (session, item) = match (state.store.find_session(&id), state.store.find_session_item(&id)) {
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  async fn resume(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.session(&id) {
          Ok(Some(s)) => s,
  ```

  with:

  ```rust
  async fn resume(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      };
      let session = match state.store.session(&id) {
          Ok(Some(s)) => s,
  ```

  with:

  ```rust
      };
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.session(&id) {
          Ok(Some(s)) => s,
  ```

  with:

  ```rust
  async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      ApiJson(req): ApiJson<ConfigRequest>,
  ) -> Response {
      let session = match state.store.session(&id) {
          Ok(Some(s)) => s,
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
  ```

  with:

  ```rust
      ApiJson(req): ApiJson<ConfigRequest>,
  ) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      match state.store.session(id) {
  ```

  with:

  ```rust
      match state.store.find_session(id) {
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  async fn park(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.session(&id) {
          Ok(Some(s)) => s,
  ```

  with:

  ```rust
  async fn park(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      let session = match state.store.session(&id) {
  ```

  with:

  ```rust
      let session = match state.store.find_session(&id) {
  ```

  In `crates/hennery-sessions/src/attachments.rs`, replace:

  ```rust

  #[cfg(test)]
  ```

  with:

  ```rust

  /// Remove the file stored as `sha256` in `dir`; one already gone is fine.
  pub fn remove(dir: &Path, sha256: &str) -> std::io::Result<()> {
      match std::fs::remove_file(path(dir, sha256)?) {
          Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
          _ => Ok(()),
      }
  }

  #[cfg(test)]
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  mod resolve;
  pub mod store;
  ```

  with:

  ```rust
  mod resolve;
  mod shared_files;
  pub mod store;
  ```

  Create `crates/hennery-sessions/src/shared_files.rs`:

  ```rust
  //! The one read across owners (plan 9a decision 6, A6): attachment files
  //! are shared by hash, so the same image sent by two owners is one file,
  //! each owner holding a row of its own (plan 6a decision 5). Whether a
  //! file may go is whether *any* owner's row still names it. This reads
  //! nothing but that existence, changes nothing, and is the owner audit's
  //! listed exemption (`crates/hennery-testkit/tests/owner_filter.rs`).

  use anyhow::Result;
  use rusqlite::Connection;

  /// The read, on `attachments_by_hash`.
  const NAMED_BY_ANY_OWNER: &str = "SELECT EXISTS(SELECT 1 FROM attachments WHERE sha256 = ?1)";

  /// Whether an `attachments` row of any owner names `sha256`.
  pub(crate) fn hash_named_by_any_owner(conn: &Connection, sha256: &str) -> Result<bool> {
      Ok(conn.query_row(NAMED_BY_ANY_OWNER, [sha256], |r| r.get(0))?)
  }

  #[cfg(test)]
  mod tests {
      /// Plan 9a: the read walks the hash's own index, not the whole table.
      #[test]
      fn the_read_uses_the_hash_index() {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          crate::store::Store::open(&db).unwrap();
          let conn = rusqlite::Connection::open(&db).unwrap();
          let plan: Vec<String> = conn
              .prepare(&format!("EXPLAIN QUERY PLAN {}", super::NAMED_BY_ANY_OWNER))
              .unwrap()
              .query_map(["0".repeat(64)], |r| r.get(3))
              .unwrap()
              .map(Result::unwrap)
              .collect();
          assert!(plan.iter().any(|p| p.contains("attachments_by_hash")), "{plan:?}");
      }
  }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  use std::collections::{BTreeMap, HashMap};
  ```

  with:

  ```rust
  use std::collections::{BTreeMap, BTreeSet, HashMap};
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  ",
  ];
  ```

  with:

  ```rust
  ",
      // Session delete (ACP core §4.10; plan 9a decisions 1, 2 and 6, A5,
      // A9). A turn's own links to the images it shows, as `event_attachments`
      // are an event's, so an image a turn shows that never started (no
      // `user_turn`) is still referenced; they go with the turn. Backfilled
      // from each turn's content as `link_attachments` links an event: an
      // image block with a hash, at its index, where the owner's row exists.
      // A block that is no object is never read as JSON (`CASE` decides
      // before `json_extract` runs; `AND` does not promise an order). Whether
      // any owner still names a file (`shared_files`) walks its hash's index.
      //
      // A deleted session (`lifecycle = 'deleted'`) is a tombstone and never
      // comes back: nothing is added for it, and its row is never changed
      // again. The store's own writers check first, so a racing writer gets
      // a typed answer; these triggers are the schema's word. A later
      // migration that UPDATEs `sessions` must exclude tombstones
      // (`lifecycle <> 'deleted'`), or this trigger aborts it.
      "
      CREATE TABLE turn_attachments (
          turn_id TEXT NOT NULL REFERENCES turns(turn_id) ON DELETE CASCADE,
          sha256 TEXT NOT NULL,
          position INTEGER NOT NULL,
          owner_id TEXT NOT NULL REFERENCES owners(id),
          PRIMARY KEY (turn_id, position),
          FOREIGN KEY (owner_id, sha256) REFERENCES attachments(owner_id, sha256));
      CREATE INDEX turn_attachments_by_image ON turn_attachments(owner_id, sha256);
      CREATE INDEX attachments_by_hash ON attachments(sha256);
      INSERT INTO turn_attachments(turn_id, sha256, position, owner_id)
          SELECT turn_id, sha256, position, owner_id FROM (
              SELECT t.turn_id, t.owner_id, b.key AS position,
                  CASE WHEN b.type = 'object' THEN
                      CASE WHEN json_extract(b.value, '$.type') = 'image' AND json_type(b.value, '$.sha256') = 'text'
                          THEN json_extract(b.value, '$.sha256') END
                  END AS sha256
              FROM turns t, json_each(t.content) b
              WHERE json_type(t.content) = 'array') linked
          WHERE sha256 IS NOT NULL
              AND EXISTS (SELECT 1 FROM attachments a WHERE a.owner_id = linked.owner_id AND a.sha256 = linked.sha256);
      CREATE TRIGGER events_of_a_tombstone BEFORE INSERT ON events
          WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
          BEGIN SELECT RAISE(ABORT, 'a deleted session gets no events'); END;
      CREATE TRIGGER turns_of_a_tombstone BEFORE INSERT ON turns
          WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
          BEGIN SELECT RAISE(ABORT, 'a deleted session gets no turns'); END;
      CREATE TRIGGER pending_of_a_tombstone BEFORE INSERT ON pending
          WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
          BEGIN SELECT RAISE(ABORT, 'a deleted session gets no questions'); END;
      CREATE TRIGGER answers_of_a_tombstone BEFORE INSERT ON answer_queue
          WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
          BEGIN SELECT RAISE(ABORT, 'a deleted session gets no answers'); END;
      CREATE TRIGGER catalog_of_a_tombstone BEFORE INSERT ON session_catalog
          WHEN EXISTS (SELECT 1 FROM sessions WHERE id = NEW.session_id AND lifecycle = 'deleted')
          BEGIN SELECT RAISE(ABORT, 'a deleted session gets no catalogue'); END;
      CREATE TRIGGER a_tombstone_stays BEFORE UPDATE ON sessions
          WHEN OLD.lifecycle = 'deleted'
          BEGIN SELECT RAISE(ABORT, 'a deleted session is never changed'); END;
  ",
  ];
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust

  /// The outcome of `Store::submit_answer` (ACP core §4.6).
  ```

  with:

  ```rust

  /// The state a delete's route judged to have no adapter it can reach
  /// (plan 9a decision 5): a session parked, failed, presumed parked, or
  /// `starting` or `active` on a host that is away. The store closes it
  /// collector-side first only while it is still exactly this (A4).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Unattached {
      pub lifecycle: String,
      pub presumed_parked: bool,
  }

  /// The outcome of `Store::delete_session` (ACP core §4.10).
  #[derive(Debug, PartialEq)]
  pub enum Deletion {
      /// Deleted: its `session_deleted` event. `unconfirmed`: it was closed
      /// here, collector-side, while its host may still run it (presumed
      /// parked, or starting or active on a host away); that host closes its
      /// adapter when it is back (plan 9a decision 4, A13). A parked or
      /// failed session runs nowhere: its close is confirmed.
      Done { event: EventDto, unconfirmed: bool },
      /// Not closed, and not what the route judged unattached: this
      /// lifecycle. Nothing changed.
      Refused(String),
      /// No such session, or a tombstone already.
      NotFound,
  }

  /// The outcome of `Store::submit_answer` (ACP core §4.6).
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      attachments: Option<PathBuf>,
  }
  ```

  with:

  ```rust
      attachments: Option<PathBuf>,
      /// The database file, for the checkpoint after a delete, which runs on
      /// a connection of its own; an in-memory store has none.
      path: Option<PathBuf>,
  }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  /// a session of `owner`'s only: for any other it fails and writes nothing.
  ```

  with:

  ```rust
  /// a session of `owner`'s only: for any other, or a tombstone (plan 9a
  /// A1), it fails and writes nothing.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
           SELECT ?1, NULL, ?2, ?3, ?4, ?5 WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?1 AND owner_id = ?5)",
  ```

  with:

  ```rust
           SELECT ?1, NULL, ?2, ?3, ?4, ?5
           WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?5)",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  /// Link a `user_turn` event to the images it shows (ACP core §8): one row
  /// per image block, at its index in the content. A block stored before
  /// plan 6a names no attachment, and links nothing (decision 9).
  fn link_attachments(tx: &Transaction<'_>, owner: &str, event_id: i64, content: &Value) -> Result<()> {
      for (position, block) in content.as_array().into_iter().flatten().enumerate() {
          if block.get("type").and_then(Value::as_str) != Some("image") {
              continue;
          }
          let Some(sha256) = block.get("sha256").and_then(Value::as_str) else {
              continue;
          };
  ```

  with:

  ```rust
  /// The image blocks of a stored content that name their image, each with
  /// its index in the content. A block stored before plan 6a names no
  /// attachment (decision 9).
  fn image_hashes<'a>(blocks: impl IntoIterator<Item = &'a Value>) -> Vec<(usize, &'a str)> {
      blocks
          .into_iter()
          .enumerate()
          .filter(|(_, block)| block.get("type").and_then(Value::as_str) == Some("image"))
          .filter_map(|(position, block)| Some((position, block.get("sha256")?.as_str()?)))
          .collect()
  }

  /// Link a `user_turn` event to the images it shows (ACP core §8): one row
  /// per image block, at its index in the content.
  fn link_attachments(tx: &Transaction<'_>, owner: &str, event_id: i64, content: &Value) -> Result<()> {
      for (position, sha256) in image_hashes(content.as_array().into_iter().flatten()) {
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          )?;
      }
      Ok(())
  }

  /// Close an open turn that the host will never end, as `interrupted`.
  ```

  with:

  ```rust
          )?;
      }
      Ok(())
  }

  /// Link a turn to the images its content shows (plan 9a decision 6), as
  /// `link_attachments` links its `user_turn`: an image a turn shows that
  /// never started is still referenced. The links go with the turn.
  fn link_turn_attachments(tx: &Transaction<'_>, owner: &str, turn_id: &str, content: &[Value]) -> Result<()> {
      for (position, sha256) in image_hashes(content) {
          tx.execute(
              "INSERT INTO turn_attachments(turn_id, sha256, position, owner_id)
               SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM attachments WHERE owner_id = ?4 AND sha256 = ?2)",
              params![turn_id, sha256, position as i64, owner],
          )?;
      }
      Ok(())
  }

  /// Delete the owner's rows of those of `hashes` that nothing of theirs
  /// shows any more, no turn and no event (plan 9a decision 6); the hashes
  /// whose row went. Their files go after the commit (`Store::remove_files`).
  fn drop_unreferenced(tx: &Transaction<'_>, owner: &str, hashes: &BTreeSet<String>) -> Result<Vec<String>> {
      let mut dropped = Vec::new();
      for sha256 in hashes {
          let gone = tx.execute(
              "DELETE FROM attachments WHERE owner_id = ?1 AND sha256 = ?2
                   AND NOT EXISTS (SELECT 1 FROM turn_attachments WHERE owner_id = ?1 AND sha256 = ?2)
                   AND NOT EXISTS (SELECT 1 FROM event_attachments WHERE owner_id = ?1 AND sha256 = ?2)",
              [owner, sha256],
          )?;
          if gone == 1 {
              dropped.push(sha256.clone());
          }
      }
      Ok(dropped)
  }

  /// Close an open turn that the host will never end, as `interrupted`.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  /// `Store::close_now`'s body, inside the caller's transaction.
  fn close_in(tx: &Transaction<'_>, owner: &str, session_id: &str) -> Result<Vec<EventDto>> {
      let row: Option<(String, bool, Option<String>)> = tx
          .query_row(
              "SELECT lifecycle, close_requested, open_turn_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
  ```

  with:

  ```rust
  /// `Store::close_now`'s body, inside the caller's transaction. A tombstone
  /// is left alone (plan 9a A1).
  fn close_in(tx: &Transaction<'_>, owner: &str, session_id: &str) -> Result<Vec<EventDto>> {
      let row: Option<(String, bool, Option<String>)> = tx
          .query_row(
              "SELECT lifecycle, close_requested, open_turn_id FROM sessions
               WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  impl Store {
      pub fn open(path: &Path) -> Result<Self> {
          let attachments = path.parent().map(|dir| dir.join(crate::attachments::DIR));
          Self::init(hennery_kernel::db::open(path)?, attachments)
      }

      pub fn open_in_memory() -> Result<Self> {
          Self::init(hennery_kernel::db::open_in_memory()?, None)
  ```

  with:

  ```rust
  /// Fold the WAL of the database at `path` back into it and truncate it, so
  /// the pages a delete wrote leave it too (plan 9a A8): best-effort, logged.
  /// On a connection of its own, so the store's lock is not held while it
  /// waits for readers; busy if one stays, and then the next checkpoint does
  /// it.
  fn checkpoint(path: &Path) {
      let checkpointed = hennery_kernel::db::open(path)
          .and_then(|conn| Ok(conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))?));
      match checkpointed {
          Ok(0) => {}
          Ok(_) => tracing::warn!("the checkpoint after a delete was busy: the WAL keeps its pages until the next"),
          Err(err) => tracing::warn!("the checkpoint after a delete failed: {err:#}"),
      }
  }

  impl Store {
      pub fn open(path: &Path) -> Result<Self> {
          let attachments = path.parent().map(|dir| dir.join(crate::attachments::DIR));
          Self::init(hennery_kernel::db::open(path)?, attachments, Some(path.to_path_buf()))
      }

      pub fn open_in_memory() -> Result<Self> {
          Self::init(hennery_kernel::db::open_in_memory()?, None, None)
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      fn init(mut conn: Connection, attachments: Option<PathBuf>) -> Result<Self> {
  ```

  with:

  ```rust
      fn init(mut conn: Connection, attachments: Option<PathBuf>, path: Option<PathBuf>) -> Result<Self> {
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              attachments,
          })
  ```

  with:

  ```rust
              attachments,
              path,
          })
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
          self.conn().execute(
              "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2 WHERE id = ?1 AND owner_id = ?3",
  ```

  with:

  ```rust
      /// Fail a session's start; a tombstone is left alone (plan 9a A1).
      pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
          self.conn().execute(
              "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2
               WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?3",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
  ```

  with:

  ```rust
      /// One session; never a tombstone (plan 9a decision 3), so every route
      /// that reads it answers 404 for a deleted session.
      pub fn find_session(&self, id: &str) -> Result<Option<SessionRow>> {
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                          git_worktree, base_commit, hat_id, agent_session_id
                   FROM sessions WHERE id = ?1 AND owner_id = ?2",
                  [id, &self.owner],
  ```

  with:

  ```rust
                          git_worktree, base_commit, hat_id, agent_session_id
                   FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                  [id, &self.owner],
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// One session as a list item, as stored (the detail's; the list serves
      /// it `bounded`).
      pub fn session_item(&self, id: &str) -> Result<Option<SessionItem>> {
          Ok(self
              .conn()
              .query_row(
                  &format!("SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE id = ?1 AND owner_id = ?2"),
  ```

  with:

  ```rust
      /// The host a session runs on, tombstones included: what a host's frame
      /// is checked against (ACP core §3.3). A frame of this host's for its
      /// deleted session goes on to `ingest`, which stores nothing, and is
      /// acked, so the host prunes its outbox (plan 9a decision 4).
      pub fn session_host(&self, id: &str) -> Result<Option<String>> {
          Ok(self
              .conn()
              .query_row(
                  "SELECT host_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
                  [id, &self.owner],
                  |r| r.get(0),
              )
              .optional()?)
      }

      /// One session as a list item, as stored (the detail's; the list serves
      /// it `bounded`); never a tombstone (plan 9a decision 3).
      pub fn find_session_item(&self, id: &str) -> Result<Option<SessionItem>> {
          Ok(self
              .conn()
              .query_row(
                  &format!(
                      "SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2"
                  ),
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// session, an empty catalogue for one whose host has reported none.
  ```

  with:

  ```rust
      /// session or a tombstone, an empty catalogue for one whose host has
      /// reported none.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                   WHERE s.id = ?1 AND s.owner_id = ?2",
  ```

  with:

  ```rust
                   WHERE s.id = ?1 AND s.lifecycle <> 'deleted' AND s.owner_id = ?2",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// stored blocks, and each image gets the owner's row, if it has none
      /// yet. Its file must be saved first (`save_images`).
  ```

  with:

  ```rust
      /// stored blocks and its links to the images (plan 9a decision 6), and
      /// each image gets the owner's row, if it has none yet. Its file must
      /// be saved first (`save_images`).
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  )?;
              }
          }
  ```

  with:

  ```rust
                  )?;
                  // A delete since `save_images` may have removed the file
                  // (plan 9a decision 6). Under the store's lock, as a delete
                  // removes files, it is written again if it is gone.
                  if let Some(dir) = self.attachments.as_deref() {
                      crate::attachments::write(dir, &image.sha256, &image.bytes)
                          .with_context(|| format!("store attachment {}", image.sha256))?;
                  }
              }
              link_turn_attachments(&tx, &self.owner, turn_id, content)?;
          }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// Undo `open_turn` after the host rejected the prompt.
  ```

  with:

  ```rust
      /// Undo `open_turn` after the host rejected the prompt. The turn's
      /// images go with it unless something else of the owner's shows them
      /// (plan 9a decision 6): their rows in this transaction, their files
      /// after it.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          )?;
          tx.execute(
  ```

  with:

  ```rust
          )?;
          let hashes: BTreeSet<String> = {
              let mut stmt = tx.prepare("SELECT sha256 FROM turn_attachments WHERE turn_id = ?1 AND owner_id = ?2")?;
              let rows = stmt.query_map([turn_id, &self.owner], |r| r.get(0))?;
              rows.collect::<rusqlite::Result<_>>()?
          };
          // Its links go with it (`ON DELETE CASCADE`).
          tx.execute(
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          Ok(())
  ```

  with:

  ```rust
          let dropped = drop_unreferenced(&tx, &self.owner, &hashes)?;
          tx.commit()?;
          self.remove_files(&conn, &dropped);
          Ok(())
      }

      /// After the commit that dropped their rows, and still under the store's
      /// lock, so no prompt records one of them meanwhile (plan 9a decision
      /// 6): remove each image's file unless a row of any owner still names
      /// it, since files are shared by hash. A file gone already is fine. One
      /// that cannot be removed is logged and left: its rows are gone, and
      /// plan 9b's sweep removes a file no row names (decision 7).
      fn remove_files(&self, conn: &Connection, dropped: &[String]) {
          let Some(dir) = self.attachments.as_deref() else {
              return;
          };
          for sha256 in dropped {
              let removed = crate::shared_files::hash_named_by_any_owner(conn, sha256).and_then(|named| {
                  if !named {
                      crate::attachments::remove(dir, sha256)?;
                  }
                  Ok(())
              });
              if let Err(err) = removed {
                  tracing::warn!(%sha256, "an unreferenced attachment's file was left: {err:#}");
              }
          }
      }

      /// Delete a session (ACP core §4.10; plan 9a decisions 1, 5 and 6), in
      /// one transaction:
      /// - a tombstone, or no session, is `NotFound`;
      /// - one not `closed` is refused with its lifecycle, unless it is still
      ///   exactly what the route judged `unattached` (A4): then it is closed
      ///   here first, as `close_now` closes, and the delete is `unconfirmed`
      ///   if its host may still run it;
      /// - its events, turns (their image links with them), questions,
      ///   answers and catalogue are deleted, and the owner's images nothing
      ///   else of theirs shows; its project recent too, unless another kept
      ///   session of that host and hat has that cwd (R1–R4);
      /// - `session_deleted` is written, and the row is scrubbed to a
      ///   tombstone: `deleted`, keeping only its id, owner, host, hat,
      ///   creation and recency.
      ///
      /// Then, still under the store's lock, the images' files that no owner
      /// names any more go; once it is released, the WAL is checkpointed so
      /// the deleted pages leave it too (A8). Both best-effort. A crash before the files go
      /// leaves files no row names, for plan 9b's sweep (decision 7).
      pub fn delete_session(&self, session_id: &str, unattached: Option<&Unattached>) -> Result<Deletion> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          // Its host, hat and cwd are read before the scrub clears them (R1).
          let row: Option<(String, bool, String, String, String)> = tx
              .query_row(
                  "SELECT lifecycle, presumed_parked, host_id, hat_id, cwd FROM sessions
                   WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                  [session_id, &self.owner],
                  |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
              )
              .optional()?;
          let Some((lifecycle, presumed, host_id, hat_id, cwd)) = row else {
              return Ok(Deletion::NotFound);
          };
          let mut unconfirmed = false;
          if lifecycle != "closed" {
              match unattached {
                  Some(judged) if judged.lifecycle == lifecycle && judged.presumed_parked == presumed => {
                      close_in(&tx, &self.owner, session_id)?;
                      // Only its host can still run it: presumed parked, or
                      // starting or active on a host the route cannot reach.
                      unconfirmed = presumed || matches!(lifecycle.as_str(), "starting" | "active");
                  }
                  _ => return Ok(Deletion::Refused(lifecycle)),
              }
          }
          let hashes: BTreeSet<String> = {
              let mut stmt = tx.prepare(
                  "SELECT sha256 FROM turn_attachments
                   WHERE owner_id = ?2 AND turn_id IN (SELECT turn_id FROM turns WHERE session_id = ?1 AND owner_id = ?2)
                   UNION
                   SELECT sha256 FROM event_attachments
                   WHERE owner_id = ?2 AND event_id IN (SELECT event_id FROM events WHERE session_id = ?1 AND owner_id = ?2)",
              )?;
              let rows = stmt.query_map([session_id, &self.owner], |r| r.get(0))?;
              rows.collect::<rusqlite::Result<_>>()?
          };
          // Children before their parents: an event's image links before the
          // event, an answer before its question. A turn's links go with it
          // (`ON DELETE CASCADE`).
          tx.execute(
              "DELETE FROM event_attachments WHERE owner_id = ?2
                   AND event_id IN (SELECT event_id FROM events WHERE session_id = ?1 AND owner_id = ?2)",
              [session_id, &self.owner],
          )?;
          tx.execute(
              "DELETE FROM answer_queue WHERE session_id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
          )?;
          tx.execute(
              "DELETE FROM pending WHERE session_id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
          )?;
          tx.execute(
              "DELETE FROM session_catalog WHERE session_id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
          )?;
          tx.execute(
              "DELETE FROM turns WHERE session_id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
          )?;
          tx.execute(
              "DELETE FROM events WHERE session_id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
          )?;
          // The kernel's table, on this transaction (R3). The path compares
          // byte for byte (`=`, the column's BINARY collation; R4), and only a
          // session kept counts (R2): this one is not yet a tombstone, so it
          // is left out by its id. A tombstone's cwd is '' already; its filter
          // guards a later change of the scrub.
          tx.execute(
              "DELETE FROM project_recents WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3 AND path = ?4
                   AND NOT EXISTS (SELECT 1 FROM sessions
                       WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3 AND cwd = ?4 AND id <> ?5
                           AND lifecycle <> 'deleted')",
              params![self.owner, host_id, hat_id, cwd, session_id],
          )?;
          let dropped = drop_unreferenced(&tx, &self.owner, &hashes)?;
          let event = collector_event(&tx, &self.owner, session_id, "session_deleted", json!({}), &now())?;
          let scrubbed = tx.execute(
              "UPDATE sessions SET lifecycle = 'deleted', cwd = '', agent = '', title = NULL, git_branch = NULL,
                   git_dirty = NULL, git_worktree = NULL, base_commit = NULL, model = NULL, mode = NULL,
                   config_axes = NULL, agent_session_id = NULL, failure_reason = NULL, hat_rule_id = NULL,
                   open_turn_id = NULL, activity = NULL, presumed_parked = 0, close_requested = 0
               WHERE id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
          )?;
          // Read in this transaction; an event without its tombstone would be
          // worse than an error.
          anyhow::ensure!(scrubbed == 1, "tombstoning {session_id} changed {scrubbed} rows");
          tx.commit()?;
          // plan 8: revoke the session's gateway tokens here
          self.remove_files(&conn, &dropped);
          drop(conn);
          if let Some(path) = self.path.as_deref() {
              checkpoint(path);
          }
          Ok(Deletion::Done { event, unconfirmed })
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// confirms, the next handshake sends `close_session` again.
  ```

  with:

  ```rust
      /// confirms, the next handshake sends `close_session` again. For a
      /// tombstone it fails as for an unknown session, and writes nothing.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              "UPDATE sessions SET close_requested = 1 WHERE id = ?1 AND owner_id = ?2",
  ```

  with:

  ```rust
              "UPDATE sessions SET close_requested = 1 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  "SELECT close_requested = 1 AND (lifecycle = 'active' OR presumed_parked = 1)
                   FROM sessions WHERE id = ?1 AND owner_id = ?2",
  ```

  with:

  ```rust
                  // A tombstone's `close_requested` is 0 already; the filter
                  // guards a later edit of the predicate.
                  "SELECT close_requested = 1 AND (lifecycle = 'active' OR presumed_parked = 1)
                   FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes, hat_id FROM sessions
                   WHERE id = ?1 AND owner_id = ?2",
                  [session_id, &self.owner],
  ```

  with:

  ```rust
                  "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes, hat_id FROM sessions
                   WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                  [session_id, &self.owner],
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  "SELECT lifecycle, presumed_parked, hat_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
  ```

  with:

  ```rust
                  "SELECT lifecycle, presumed_parked, hat_id FROM sessions
                   WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  "SELECT id FROM sessions WHERE host_id = ?1 AND lifecycle = 'active' AND owner_id = ?2 ORDER BY id",
  ```

  with:

  ```rust
                  "SELECT id FROM sessions
                   WHERE host_id = ?1 AND lifecycle = 'active' AND lifecycle <> 'deleted' AND owner_id = ?2 ORDER BY id",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              let mut stmt =
                  tx.prepare("SELECT id FROM sessions WHERE host_id = ?1 AND lifecycle = 'starting' AND owner_id = ?2")?;
  ```

  with:

  ```rust
              let mut stmt = tx.prepare(
                  "SELECT id FROM sessions
                       WHERE host_id = ?1 AND lifecycle = 'starting' AND lifecycle <> 'deleted' AND owner_id = ?2",
              )?;
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                   WHERE host_id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1) AND owner_id = ?2 ORDER BY id",
  ```

  with:

  ```rust
                   WHERE host_id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1) AND lifecycle <> 'deleted'
                       AND owner_id = ?2
                   ORDER BY id",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              "SELECT DISTINCT host_id FROM sessions WHERE lifecycle = 'active' AND owner_id = ?1 ORDER BY host_id",
  ```

  with:

  ```rust
              "SELECT DISTINCT host_id FROM sessions
               WHERE lifecycle = 'active' AND lifecycle <> 'deleted' AND owner_id = ?1 ORDER BY host_id",
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// not the owner's fails, and nothing is written.
      pub fn ingest_fact(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Ingested> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let owned: bool = tx.query_row(
              "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND owner_id = ?2)",
              [session_id, &self.owner],
              |r| r.get(0),
          )?;
          anyhow::ensure!(owned, "no session {session_id}");
  ```

  with:

  ```rust
      /// not the owner's fails, and nothing is written. One for a tombstone
      /// creates nothing and stores nothing, and is not an error (plan 9a
      /// decision 4, A1): the host's frame is acked, so it prunes its outbox.
      pub fn ingest_fact(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Ingested> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let lifecycle: Option<String> = tx
              .query_row(
                  "SELECT lifecycle FROM sessions WHERE id = ?1 AND owner_id = ?2",
                  [session_id, &self.owner],
                  |r| r.get(0),
              )
              .optional()?;
          let Some(lifecycle) = lifecycle else {
              anyhow::bail!("no session {session_id}");
          };
          if lifecycle == "deleted" {
              return Ok(Ingested {
                  events: Vec::new(),
                  edge: None,
              });
          }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// - attached sessions the operator closed → returned in `close`.
  ```

  with:

  ```rust
      /// - attached sessions the operator closed or deleted → returned in
      ///   `close`.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                   WHERE host_id = ?1 AND (lifecycle IN ('starting', 'active', 'closed') OR presumed_parked = 1)
  ```

  with:

  ```rust
                   WHERE host_id = ?1 AND (lifecycle IN ('starting', 'active', 'closed', 'deleted') OR presumed_parked = 1)
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  ("closed", Some(_)) => out.close.push(id),
  ```

  with:

  ```rust
                  // Nothing is written for a tombstone (plan 9a decision 4): its
                  // host is told to close the adapter, and its answer, a
                  // `session_closed` or `not_attached`, changes nothing.
                  ("closed" | "deleted", Some(_)) => out.close.push(id),
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust

      /// The kernel's first two migrations, as 3b-ii shipped them.
  ```

  with:

  ```rust

      /// Plan 9a decision 6: the turn links are backfilled from each turn's
      /// content, as `link_attachments` links an event's: an image block with
      /// a hash, at its index, only where the owner's row for it exists.
      /// Anything else in the content (a block that is no object, an image
      /// with no hash, a hash with no row) links nothing and fails nothing.
      #[test]
      fn the_turn_links_are_backfilled_from_the_turns_content() {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let (a, b, gone) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
          {
              let mut conn = hennery_kernel::db::open(&db).unwrap();
              let owner = hennery_kernel::db::kernel_owner(&mut conn).unwrap();
              // Every migration before plan 9a's, wherever later lanes put it.
              let before = MIGRATIONS
                  .iter()
                  .position(|m| m.contains("CREATE TABLE turn_attachments"))
                  .unwrap();
              hennery_kernel::db::migrate(&mut conn, &MIGRATIONS[..before]).unwrap();
              let content = json!([
                  { "type": "text", "text": "x" },
                  { "type": "image", "mimeType": "image/png", "sha256": a },
                  "a string",
                  { "type": "image", "mimeType": "image/png" },
                  { "type": "image", "mimeType": "image/png", "sha256": gone },
                  { "type": "image", "mimeType": "image/png", "sha256": 7 },
                  { "type": "image", "mimeType": "image/png", "sha256": b },
                  { "type": "image", "mimeType": "image/png", "sha256": a },
              ]);
              conn.execute_batch(&format!(
                  "
                  INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id)
                      VALUES ('s1', 'h1', 'fake', '/tmp', 'active', 't', 't', '{owner}');
                  INSERT INTO attachments(owner_id, sha256, mime, size, created_at) VALUES
                      ('{owner}', '{a}', 'image/png', 1, 't'), ('{owner}', '{b}', 'image/png', 1, 't');
                  INSERT INTO turns(turn_id, session_id, content, created_at, owner_id) VALUES
                      ('t1', 's1', '{content}', 't', '{owner}'),
                      ('t2', 's1', '[]', 't', '{owner}'),
                      ('t3', 's1', '{{\"not\": \"a list\"}}', 't', '{owner}');
                  "
              ))
              .unwrap();
          }
          let store = Store::open(&db).unwrap();
          let conn = Connection::open(&db).unwrap();
          let mut stmt = conn
              .prepare("SELECT turn_id, sha256, position, owner_id FROM turn_attachments ORDER BY turn_id, position")
              .unwrap();
          let links: Vec<(String, String, i64, String)> = stmt
              .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
              .unwrap()
              .map(Result::unwrap)
              .collect();
          let owner = store.owner_id().to_string();
          assert_eq!(
              links,
              [
                  ("t1".into(), a.clone(), 1, owner.clone()),
                  ("t1".into(), b, 6, owner.clone()),
                  ("t1".into(), a, 7, owner),
              ]
          );
      }

      /// The kernel's first two migrations, as 3b-ii shipped them.
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              assert!(store.session("session-old").unwrap().is_some());
  ```

  with:

  ```rust
              assert!(store.find_session("session-old").unwrap().is_some());
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                  match state.store.session(&session_id) {
  ```

  with:

  ```rust
                  match state.store.find_session(&session_id) {
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
      let (cwd, hat_id) = match state.store.session(session_id) {
  ```

  with:

  ```rust
      let (cwd, hat_id) = match state.store.find_session(session_id) {
  ```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-sessions --locked`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

Each change is made on the task's code, the named test is run, and the change is restored. Every one was caught:
- **The delete transaction:**
  - removing each `DELETE` (event_attachments, answer_queue, pending, session_catalog, turns, events), `drop_unreferenced`, the `session_deleted` event, or `remove_files` fails `a_deleted_session_leaves_only_its_tombstone_row_and_one_event`;
  - removing the compare-and-set on the lifecycle, or on `presumed_parked`, fails `a_session_not_closed_is_deleted_only_as_the_route_judged_it`;
  - removing the recents `DELETE`, its `NOT EXISTS`, or its `id <> ?5` fails `a_delete_removes_its_recent_unless_another_kept_session_has_that_cwd`.
- **The scrub:** removing the whole `UPDATE`, or any one of its columns, fails `a_tombstone_is_scrubbed_and_found_by_no_accessor_or_list`. So does removing the `find_session` or `find_session_item` filter.
- **The files:**
  - removing `secure_delete`, or the checkpoint, fails `a_deleted_sessions_title_and_cwd_are_not_left_in_the_database_files`;
  - removing the `hash_named_by_any_owner` check fails `an_image_file_another_owner_holds_stays_when_the_owners_row_goes`;
  - removing either `NOT EXISTS` of `drop_unreferenced` fails the two image-reference tests;
  - removing `abandon_turn`'s clean-up fails `abandoning_a_turn_removes_an_image_only_it_showed`.
- **The schema:**
  - removing each of the six triggers fails `the_schema_refuses_writes_for_a_tombstone`;
  - removing the backfill's `EXISTS` or its object `CASE` fails `the_turn_links_are_backfilled_from_the_turns_content`.
- **The guards:**
  - removing the `collector_event`, `ingest`, `close_in`, `mark_failed`, `catalog`, `record_close_request`, `request_resume` or `reassign_hat` guard fails `the_stores_writers_leave_a_tombstone_alone`;
  - removing reconcile's `'deleted'`, or its close arm, fails `a_tombstone_goes_through_the_bulk_functions_untouched`;
  - removing `attachments_by_hash` fails `the_read_uses_the_hash_index`.
- **Not probed, kept as defence in depth:** the `<> 'deleted'` in the scans whose predicate already excludes a tombstone (each carries a comment), and `close_in` inside the delete (all it writes is deleted in the same transaction).

- [ ] **Step 6: The full checks**

Expected: all pass; **986 tests** on `4031ba0`.

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(sessions): delete a session in the store, leaving a scrubbed tombstone"
```

### Task 2: The host side: ack, discard, and close on return

**Files:**
- Modify: `crates/hennery-sessions/src/ws.rs`
- Test: `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: Task 1's `session_host`, `ingest`'s tombstone path, and reconcile's close of a tombstone.

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      seq: u64,
  }
  ```

  with:

  ```rust
      seq: u64,
      /// The collector's `hello_ack`, once `hello` has had it.
      hello_ack: Option<CollectorFrame>,
  }
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          let mut host = Self { ws, seq };
  ```

  with:

  ```rust
          let mut host = Self {
              ws,
              seq,
              hello_ack: None,
          };
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
          host
  ```

  with:

  ```rust
          assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
          host.hello_ack = Some(ack);
          host
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          let mut host = Self { ws, seq: 0 };
  ```

  with:

  ```rust
          let mut host = Self {
              ws,
              seq: 0,
              hello_ack: None,
          };
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
                      },
                      Some(Ok(_)) => {}
  ```

  with:

  ```rust
                      },
                      Some(Ok(_)) => {}
                      other => panic!("collector connection ended: {other:?}"),
                  }
              }
          })
          .await
          .expect("a collector frame within 10s")
      }

      /// The next collector frame, an `ack` included.
      async fn next_frame(&mut self) -> CollectorFrame {
          tokio::time::timeout(Duration::from_secs(10), async {
              loop {
                  match self.ws.next().await {
                      Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                      Some(Ok(_)) => {}
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      .await;
      assert!(collector.notices().is_empty());
      host.send(&HostFrame::ResendComplete).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
  }
  ```

  with:

  ```rust
      .await;
      assert!(collector.notices().is_empty());
      host.send(&HostFrame::ResendComplete).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
  }

  // Plan 9a: a deleted session on its host (ACP core §4.10; decision 4).

  /// A session deleted while its host was away never comes back: everything
  /// the host still sends for it is acked, so its outbox is pruned, and
  /// discarded; the reconnected host is told to close its adapter, and its
  /// `session_closed` is acked and discarded too. A frame for another host's
  /// tombstone is neither acked nor stored, as for any session not its own.
  #[tokio::test]
  async fn a_session_deleted_while_its_host_was_away_is_closed_and_never_comes_back() {
      use hennery_sessions::store::{Deletion, ListQuery, Unattached};
      let collector = Collector::start_in(tempfile::tempdir().unwrap(), Duration::from_millis(300)).await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let turn = started_turn(&collector, &mut host, &session).await;
      let seq = host.seq;
      host.drop_connection(&collector).await;
      presumed_parked(&collector, &session).await;

      // The operator deletes it, as the route will: presumed parked, so it is
      // closed collector-side first.
      let store = &collector.state.store;
      let Deletion::Done { unconfirmed, .. } = store
          .delete_session(
              &session,
              Some(&Unattached {
                  lifecycle: "parked".into(),
                  presumed_parked: true,
              }),
          )
          .unwrap()
      else {
          panic!("not deleted");
      };
      assert!(unconfirmed);
      // Another host's tombstone, which this host has no business writing to.
      store
          .create_session("s-elsewhere", "host-2", "fake", "/tmp", "hat-x", None)
          .unwrap();
      store.close_now("s-elsewhere").unwrap();
      assert!(matches!(
          store.delete_session("s-elsewhere", None).unwrap(),
          Deletion::Done { .. }
      ));
      let only_tombstone = |id: &str| {
          let kinds = collector.event_kinds(id);
          assert_eq!(kinds, ["session_deleted"], "{id}");
      };
      only_tombstone(&session);

      // The host comes back with the session still attached, and resends
      // what its outbox holds for it.
      let listed = AttachedSession {
          open_turn_id: Some(turn.clone()),
          ..attached(&session, seq)
      };
      let mut host = ScriptedHost::hello(&collector, vec![listed], seq).await;
      let Some(CollectorFrame::HelloAck { committed, .. }) = host.hello_ack.clone() else {
          panic!("no hello_ack");
      };
      assert_eq!(committed.get(&session), Some(&0), "{committed:?}");
      host.emit(
          &session,
          SessionBody::AcpUpdate {
              indexed: Default::default(),
              payload: json!({ "update": { "sessionUpdate": "agent_message_chunk" } }),
          },
      )
      .await;
      host.emit(
          &session,
          turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
      )
      .await;
      for at in [seq + 1, seq + 2] {
          let frame = host.next_frame().await;
          assert!(
              matches!(&frame, CollectorFrame::Ack { session_id, ack_seq } if *session_id == session && *ack_seq == at),
              "{frame:?}"
          );
      }
      host.send(&HostFrame::ResendComplete).await;
      let CollectorFrame::CloseSession { session_id, .. } = host.next().await else {
          panic!("expected close_session");
      };
      assert_eq!(session_id, session);
      host.emit(&session, SessionBody::SessionClosed).await;
      let frame = host.next_frame().await;
      assert!(
          matches!(&frame, CollectorFrame::Ack { session_id, ack_seq } if *session_id == session && *ack_seq == seq + 3),
          "{frame:?}"
      );

      // Another host's tombstone: not acked. Acks come in order, so the next
      // one is the frame after it.
      host.emit("s-elsewhere", SessionBody::SessionClosed).await;
      host.emit(&session, SessionBody::SessionClosed).await;
      let frame = host.next_frame().await;
      assert!(
          matches!(&frame, CollectorFrame::Ack { session_id, ack_seq } if *session_id == session && *ack_seq == seq + 5),
          "{frame:?}"
      );

      only_tombstone(&session);
      only_tombstone("s-elsewhere");
      assert_eq!(store.committed_seq(&session).unwrap(), 0);
      assert_eq!(store.find_session(&session).unwrap(), None);
      assert!(store.list(&ListQuery::default()).unwrap().sessions.is_empty());
  }

  /// Decision 4: a host that answers the close of a deleted session
  /// `not_attached` changes nothing, and is told again on its next return.
  #[tokio::test]
  async fn a_not_attached_answer_for_a_deleted_session_changes_nothing() {
      use hennery_sessions::store::{Deletion, Unattached};
      let collector = Collector::start_in(tempfile::tempdir().unwrap(), Duration::from_millis(300)).await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let seq = host.seq;
      host.drop_connection(&collector).await;
      presumed_parked(&collector, &session).await;
      let unattached = Unattached {
          lifecycle: "parked".into(),
          presumed_parked: true,
      };
      assert!(matches!(
          collector
              .state
              .store
              .delete_session(&session, Some(&unattached))
              .unwrap(),
          Deletion::Done { .. }
      ));

      for _ in 0..2 {
          let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
          let CollectorFrame::CloseSession { request_id, session_id } = host.next().await else {
              panic!("expected close_session");
          };
          assert_eq!(session_id, session);
          host.send(&HostFrame::Error {
              request_id,
              code: "not_attached".into(),
              message: "no such session".into(),
          })
          .await;
          // A frame after it, acked, shows the answer was read.
          host.emit(&session, SessionBody::SessionClosed).await;
          loop {
              if let CollectorFrame::Ack { .. } = host.next_frame().await {
                  break;
              }
          }
          assert_eq!(collector.event_kinds(&session), ["session_deleted"]);
          host.drop_connection(&collector).await;
      }
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test reconcile deleted`
Expected: FAIL: the resent `acp_update` of the tombstone is never acked, because ws.rs still treats it as another host's session.

- [ ] **Step 3: ws.rs sees this host's tombstones**

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                  // discard forever (ACP core §3.3, §5).
                  match state.store.find_session(&session_id) {
                      Ok(Some(row)) if row.host_id == host_id => {}
  ```

  with:

  ```rust
                  // discard forever (ACP core §3.3, §5). Its deleted sessions
                  // are still its own: `ingest` stores nothing for them, and
                  // the frame is acked, so the host prunes it (plan 9a
                  // decision 4).
                  match state.store.session_host(&session_id) {
                      Ok(Some(owner)) if owner == host_id => {}
  ```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-testkit --locked --test reconcile`
Expected: PASS. The two new tests depend on timing (a 300 ms offline threshold, polled). They passed with 4 copies of the binary in parallel, three rounds.

- [ ] **Step 5: Revert-probes**

- Going back to `find_session` in ws.rs fails `a_session_deleted_while_its_host_was_away_is_closed_and_never_comes_back`.
- Removing `ingest`'s tombstone check makes the trigger fail ingest. The connection then drops without acking, and the same test fails.
- Removing reconcile's close arm fails `a_not_attached_answer_for_a_deleted_session_changes_nothing`.

- [ ] **Step 6: The full checks**

Expected: all pass; **988 tests** on `4031ba0`.

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(sessions): acknowledge and close a deleted session's frames on its host"
```

### Task 3: `DELETE /api/sessions/{id}`, the streams, and the reconnect race

**Files:**
- Modify: `crates/hennery-sessions/src/{api,ws,store}.rs`
- Test: `crates/hennery-testkit/tests/{reconcile,images,step_up,auth,owner_filter}.rs`; the unit tests in `api.rs`

**Interfaces:**
- Produces:
  - `DELETE /api/sessions/{id}` → 204;
  - the 404s on the session's events and stream;
  - `Store::tombstones_of`;
  - `ws::after_reconcile` and `ws::ready`.
- Consumes: Task 1's `delete_session`.

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust

  /// Decision 6: an image is the owner's while a turn or an event of a kept
  ```

  with:

  ```rust

  /// A8: a reader that holds the WAL through the delete's checkpoint makes it
  /// busy; once the reader is gone, the checkpoint is retried and the
  /// deleted title and cwd leave the WAL too.
  #[test]
  fn a_checkpoint_a_reader_held_up_is_retried_once_it_is_gone() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let (title, cwd) = ("zq-held-title-4d1a", "/srv/zq-held-cwd-8b2c");
      active(&store, "s1", cwd);
      store.ingest("s1", 2, &titled(title)).unwrap();
      store.close_now("s1").unwrap();
      // A read transaction on another connection, open across the delete.
      let reader = Connection::open(&db).unwrap();
      reader.execute_batch("BEGIN").unwrap();
      let _: i64 = reader
          .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
          .unwrap();
      delete(&store, "s1");
      let wal = || std::fs::read(db.with_extension("db-wal")).unwrap_or_default();
      let holds = |bytes: &[u8], needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
      assert!(holds(&wal(), title), "the reader held the checkpoint up");
      reader.execute_batch("COMMIT").unwrap();
      drop(reader);
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
      while holds(&wal(), title) || holds(&wal(), cwd) {
          assert!(std::time::Instant::now() < deadline, "the checkpoint was not retried");
          std::thread::sleep(std::time::Duration::from_millis(100));
      }
      drop(store);
  }

  /// A8: a reader held past the retries' deadline. The delete still
  /// succeeds; the debt is recorded beside the database; once the reader
  /// is released, a later retry completes the checkpoint with no other
  /// delete, and the record goes.
  #[test]
  fn a_checkpoint_a_reader_holds_past_the_deadline_is_owed_until_it_completes() {
      use hennery_sessions::store::CheckpointPolicy;
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      store.set_checkpoint_policy(CheckpointPolicy {
          retry: std::time::Duration::from_millis(20),
          // No fast retries: past the deadline at once, so only the slow
          // ones can pay the debt.
          fast_retries: 0,
          slow_retry: std::time::Duration::from_millis(100),
      });
      let (title, cwd) = ("zq-owed-title-6e0f", "/srv/zq-owed-cwd-1c9d");
      active(&store, "s1", cwd);
      store.ingest("s1", 2, &titled(title)).unwrap();
      store.close_now("s1").unwrap();
      let reader = Connection::open(&db).unwrap();
      reader.execute_batch("BEGIN").unwrap();
      let _: i64 = reader
          .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
          .unwrap();
      delete(&store, "s1");
      let marker = dir.path().join("hennery.db-checkpoint-owed");
      let wal = || std::fs::read(db.with_extension("db-wal")).unwrap_or_default();
      let holds = |bytes: &[u8], needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
      assert!(marker.exists(), "the debt is recorded");
      // Held well past the deadline: the slow retries keep going.
      std::thread::sleep(std::time::Duration::from_secs(5));
      assert!(holds(&wal(), title) && marker.exists(), "still held, still owed");
      reader.execute_batch("COMMIT").unwrap();
      drop(reader);
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
      while holds(&wal(), title) || holds(&wal(), cwd) || marker.exists() {
          assert!(
              std::time::Instant::now() < deadline,
              "the owed checkpoint never completed"
          );
          std::thread::sleep(std::time::Duration::from_millis(50));
      }
  }

  /// A8: a checkpoint owed at a restart (the record left beside the
  /// database) is paid when the store opens.
  #[test]
  fn a_checkpoint_owed_at_a_restart_is_paid_when_the_store_opens() {
      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      drop(Store::open(&db).unwrap());
      // A connection that stays open keeps the WAL from being removed at
      // close, as a crash would leave it.
      let keep = Connection::open(&db).unwrap();
      keep.execute_batch(
          "PRAGMA wal_autocheckpoint = 0; CREATE TABLE scratch(x); INSERT INTO scratch VALUES ('zq-owed-at-start');",
      )
      .unwrap();
      let marker = dir.path().join("hennery.db-checkpoint-owed");
      std::fs::write(&marker, b"").unwrap();
      assert!(!std::fs::read(db.with_extension("db-wal")).unwrap().is_empty());
      let _store = Store::open(&db).unwrap();
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
      while marker.exists()
          || !std::fs::read(db.with_extension("db-wal"))
              .unwrap_or_default()
              .is_empty()
      {
          assert!(
              std::time::Instant::now() < deadline,
              "the owed checkpoint was not paid at start"
          );
          std::thread::sleep(std::time::Duration::from_millis(50));
      }
      drop(keep);
  }

  /// Decision 6: an image is the owner's while a turn or an event of a kept
  ```

  In `crates/hennery-testkit/tests/auth.rs`, replace:

  ```rust
      ("GET", "/api/sessions/s-1"),
      ("POST", "/api/sessions/s-1/resume"),
  ```

  with:

  ```rust
      ("GET", "/api/sessions/s-1"),
      ("DELETE", "/api/sessions/s-1"),
      ("POST", "/api/sessions/s-1/resume"),
  ```

  In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
      assert_eq!(resp.status(), 401);
  }
  ```

  with:

  ```rust
      assert_eq!(resp.status(), 401);
  }

  /// Plan 9a decision 5: deleting an active session on a connected host
  /// closes it there first, then deletes it: 204, 404 after, off the list, its
  /// image no longer served, its file gone and the usage down.
  #[tokio::test]
  async fn deleting_an_active_session_closes_it_on_its_host_then_removes_it_and_its_images() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, Capabilities(vec![Capability::Park, Capability::Images])).await;
      let session = started_session(&collector, &mut host).await;
      let bytes = png(9, 4096);
      let c = client(&collector);
      let c2 = c.clone();
      let url = prompt_url(&collector, &session);
      let body = json!({ "content": [{ "type": "text", "text": "look" }, image("image/png", &bytes)] });
      let call = tokio::spawn(async move { post(&c2, url, &body).await });
      accept_prompt(&mut host, &session).await;
      assert_eq!(call.await.unwrap().0, 202);
      assert_eq!(collector.files(), [sha(&bytes)]);
      let served = collector.url(&format!("/api/attachments/{}", sha(&bytes)));
      assert_eq!(c.get(&served).send().await.unwrap().status(), 200);
      assert_eq!(
          usage(&c, &collector).await,
          AttachmentUsage {
              count: 1,
              bytes: bytes.len() as u64
          }
      );

      let c2 = c.clone();
      let url = collector.url(&format!("/api/sessions/{session}"));
      let call = tokio::spawn(async move { c2.delete(url).timeout(Duration::from_secs(60)).send().await.unwrap() });
      let CollectorFrame::CloseSession { session_id, .. } = host.next().await else {
          panic!("expected close_session");
      };
      assert_eq!(session_id, session);
      host.emit(&session, SessionBody::SessionClosed).await;
      assert_eq!(call.await.unwrap().status(), 204);

      for path in [
          format!("/api/sessions/{session}"),
          format!("/api/sessions/{session}/events"),
          format!("/api/stream/sessions/{session}"),
      ] {
          let status = c.get(collector.url(&path)).send().await.unwrap().status();
          assert_eq!(status, 404, "{path}");
      }
      let list: Value = c
          .get(collector.url("/api/sessions"))
          .send()
          .await
          .unwrap()
          .json()
          .await
          .unwrap();
      assert_eq!(list["sessions"], json!([]), "{list}");
      assert!(collector.files().is_empty(), "{:?}", collector.files());
      assert_eq!(c.get(&served).send().await.unwrap().status(), 404);
      assert_eq!(usage(&c, &collector).await, AttachmentUsage { count: 0, bytes: 0 });
  }

  /// `GET /api/settings/attachments`.
  async fn usage(c: &reqwest::Client, collector: &Collector) -> AttachmentUsage {
      c.get(collector.url("/api/settings/attachments"))
          .send()
          .await
          .unwrap()
          .json()
          .await
          .unwrap()
  }
  ```

  In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          105,
  ```

  with:

  ```rust
          106,
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          host.drop_connection(&collector).await;
      }
  }
  ```

  with:

  ```rust
          host.drop_connection(&collector).await;
      }
  }

  // Plan 9a: `DELETE /api/sessions/{id}` (decision 5, A10).

  async fn delete(c: &reqwest::Client, url: String) -> (u16, Value) {
      let resp = c.delete(url).timeout(Duration::from_secs(15)).send().await.unwrap();
      let status = resp.status().as_u16();
      (status, resp.json().await.unwrap_or(Value::Null))
  }

  fn session_url(collector: &Collector, session: &str) -> String {
      collector.url(&format!("/api/sessions/{session}"))
  }

  /// Decision 5: a parked session has no adapter: deleted at once.
  #[tokio::test]
  async fn deleting_a_parked_session_deletes_it_at_once() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = parked_session(&collector, &mut host).await;
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      assert_eq!(get(&client(&collector), session_url(&collector, &session)).await.0, 404);
      // Deleted again: not found.
      assert_eq!(
          delete(&client(&collector), session_url(&collector, &session)).await.0,
          404
      );
      assert_eq!(
          delete(&client(&collector), session_url(&collector, "s-nope")).await.0,
          404
      );
  }

  /// Decision 5: a closed session is deleted as it is.
  #[tokio::test]
  async fn deleting_a_closed_session_deletes_it() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = parked_session(&collector, &mut host).await;
      let close = collector.url(&format!("/api/sessions/{session}/close"));
      assert_eq!(post(&client(&collector), close, json!({})).await.0, 202);
      assert_eq!(collector.lifecycle(&session), "closed");
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      assert_eq!(get(&client(&collector), session_url(&collector, &session)).await.0, 404);
  }

  /// Decision 5: an active session whose host answers the close
  /// `not_attached` no longer runs there: closed collector-side, as judged
  /// (A4), and deleted.
  #[tokio::test]
  async fn a_delete_whose_close_is_answered_not_attached_deletes_it() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let CollectorFrame::CloseSession { request_id, .. } = host.next().await else {
          panic!("expected close_session");
      };
      host.send(&HostFrame::Error {
          request_id,
          code: "not_attached".into(),
          message: "no such session".into(),
      })
      .await;
      let (status, body) = call.await.unwrap();
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
  }

  /// Decision 5, 4: a session presumed parked while its host is away is
  /// closed collector-side and deleted; the host is told to close it on its
  /// return.
  #[tokio::test]
  async fn deleting_a_session_of_a_host_away_answers_at_once_and_its_host_closes_it_on_return() {
      let collector = Collector::start_in(tempfile::tempdir().unwrap(), Duration::from_millis(300)).await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let seq = host.seq;
      host.drop_connection(&collector).await;
      presumed_parked(&collector, &session).await;
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
      let CollectorFrame::CloseSession { session_id, .. } = host.next().await else {
          panic!("expected close_session");
      };
      assert_eq!(session_id, session);
  }

  /// Decision 5: a start in flight on a reachable host is refused, as close
  /// refuses it.
  #[tokio::test]
  async fn deleting_a_starting_session_on_a_reachable_host_is_refused() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let c = client(&collector);
      let url = collector.url("/api/sessions");
      let _start =
          tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
      let CollectorFrame::StartSession { session_id, .. } = host.next().await else {
          panic!("expected start_session");
      };
      let (status, body) = delete(&client(&collector), session_url(&collector, &session_id)).await;
      assert_eq!((status, body["code"].as_str()), (409, Some("starting")), "{body}");
      assert_eq!(collector.lifecycle(&session_id), "starting");
  }

  /// Decision 5: a close whose delivery is unknown (the host went away with
  /// it) answers 503 and deletes nothing; the close stays requested.
  #[tokio::test]
  async fn a_delete_whose_close_delivery_is_unknown_deletes_nothing() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let CollectorFrame::CloseSession { .. } = host.next().await else {
          panic!("expected close_session");
      };
      host.drop_connection(&collector).await;
      let (status, body) = call.await.unwrap();
      assert_eq!(
          (status, body["code"].as_str()),
          (503, Some("delivery_unknown")),
          "{body}"
      );
      let row = collector
          .state
          .store
          .find_session(&session)
          .unwrap()
          .expect("not deleted");
      assert!(row.close_requested);
  }

  /// A10: an open stream of the session gets its `session_deleted` and ends;
  /// afterwards the stream and the events answer 404.
  #[tokio::test]
  async fn an_open_stream_gets_session_deleted_and_ends() {
      use futures::StreamExt;
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = parked_session(&collector, &mut host).await;
      let resp = client(&collector)
          .get(collector.url(&format!("/api/stream/sessions/{session}")))
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 200);
      let mut body = resp.bytes_stream();
      assert_eq!(
          delete(&client(&collector), session_url(&collector, &session)).await.0,
          204
      );
      let mut buf = String::new();
      tokio::time::timeout(Duration::from_secs(10), async {
          while let Some(chunk) = body.next().await {
              buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
          }
      })
      .await
      .unwrap_or_else(|_| panic!("the stream stayed open: {buf}"));
      assert!(buf.contains("\"kind\":\"session_deleted\""), "{buf}");
      for path in [
          format!("/api/stream/sessions/{session}"),
          format!("/api/sessions/{session}/events"),
      ] {
          let (status, body) = get(&client(&collector), collector.url(&path)).await;
          assert_eq!((status, body["code"].as_str()), (404, Some("not_found")), "{path}");
      }
  }
  ```

  In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
  //! push (plan 10a decision 4) and revoking a session need
  ```

  with:

  ```rust
  //! push (plan 10a decision 4), deleting a session (plan 9a decision 5) and
  //! revoking a signed-in session need
  ```

  In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
          ("DELETE", "/api/hosts/host-9".to_string(), None, 404),
          ("DELETE", format!("/api/auth/sessions/{}", c.id_of(&other)), None, 204),
  ```

  with:

  ```rust
          ("DELETE", "/api/hosts/host-9".to_string(), None, 404),
          // Checked before anything is read: an unknown session is 403 when
          // stale, 404 only once stepped up (plan 9a decision 5).
          ("DELETE", "/api/sessions/s-9".to_string(), None, 404),
          ("DELETE", format!("/api/auth/sessions/{}", c.id_of(&other)), None, 204),
  ```

  In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
      // A PATCH that names no hat needs no step-up (plan 5d decision 2).
      let resp = send(&stale, "PATCH", "/api/sessions/s-9", Some("{}")).await.unwrap();
  ```

  with:

  ```rust
      // A stale DELETE deletes nothing (plan 9a decision 5).
      c.state
          .store
          .create_session("s-kept", "host-9", "fake", "/tmp", "hat-9", None)
          .unwrap();
      c.state.store.close_now("s-kept").unwrap();
      let resp = send(&stale, "DELETE", "/api/sessions/s-kept", None).await.unwrap();
      assert_eq!(code_of(resp).await, (403, "step_up_required".into()));
      assert!(c.state.store.find_session("s-kept").unwrap().is_some());
      // A PATCH that names no hat needs no step-up (plan 5d decision 2), nor
      // does a GET of the path DELETE shares (plan 9a decision 5).
      let resp = send(&stale, "PATCH", "/api/sessions/s-9", Some("{}")).await.unwrap();
      assert_eq!(resp.status(), 404);
      let resp = send(&stale, "GET", "/api/sessions/s-9", None).await.unwrap();
  ```

  In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
  /// Open `GET /api/stream/sessions/s-1` with `session`.
  async fn open_stream(c: &Collector, session: &str) -> reqwest::Response {
  ```

  with:

  ```rust
  /// Open `GET /api/stream/sessions/s-1` with `session`; `s-1` is made if
  /// it is not there yet (A10: an unknown session's stream is 404).
  async fn open_stream(c: &Collector, session: &str) -> reqwest::Response {
      if c.state.store.find_session("s-1").unwrap().is_none() {
          c.state
              .store
              .create_session("s-1", "host-9", "fake", "/tmp", "hat-9", None)
              .unwrap();
      }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test reconcile --test images --test step_up`
Expected: FAIL: `DELETE /api/sessions/…` answers 405, and the step-up row expects 403.

- [ ] **Step 3: The route**

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      AnswerSubmission, Cursor, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, Reassign, ResumeRequest, Store,
  };
  use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
  ```

  with:

  ```rust
      AnswerSubmission, Cursor, Deletion, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, Reassign,
      ResumeRequest, SessionRow, Store, Unattached,
  };
  use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
  use axum::handler::Handler;
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  use axum::{Json, Router};
  ```

  with:

  ```rust
  use axum::{Json, Router, middleware};
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          .route("/api/sessions/{id}", get(session_detail).patch(update_session))
  ```

  with:

  ```rust
          // Step-up is layered on `delete` alone (`Handler::layer`, as in
          // hats.rs), so GET and PATCH are as they were; PATCH checks it
          // itself when it names a hat (plan 5d decision 2).
          .route(
              "/api/sessions/{id}",
              get(session_detail)
                  .patch(update_session)
                  .delete(delete_session.layer(middleware::from_fn(hennery_kernel::auth::require_step_up))),
          )
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  /// Close: attached sessions are closed by their host (`session_closed`);
  /// anything else is closed immediately. A close whose delivery is unknown
  /// stays requested and is re-sent after the host's next handshake.
  async fn close(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
          Err(err) => return internal(err),
      };
      let reachable = state.hub.is_ready(&session.host_id);
      match session.lifecycle.as_str() {
          "closed" => return lifecycle_response(&state, &id),
          "starting" if reachable => {
              return error(
                  StatusCode::CONFLICT,
                  "starting",
                  "the session is starting; close it once the start settles",
              );
          }
          "active" if reachable => {}
          _ => return close_unattached(&state, &id),
      }
      match state.store.record_close_request(&id) {
          Ok(event) => state.hub.publish(event),
          Err(err) => return internal(err),
  ```

  with:

  ```rust
  /// Where closing a session stands (ACP core §4.8), for `close` and
  /// `delete` (plan 9a decision 5).
  pub(crate) enum Closing {
      /// Closed: by its host just now, or already.
      Closed,
      /// It has no adapter the collector can reach, as read: to be closed
      /// collector-side, while it is still this (A4).
      Unattached(Unattached),
      /// Refused, or the host's answer is not known: this response.
      Answer(Response),
  }

  /// Close `session` through its host if it is attached there: a start in
  /// flight on a reachable host is refused; an active session on a reachable
  /// host gets a durable close request and `close_session`, waiting for its
  /// end. A host that answers `not_attached` no longer has it: it is closed
  /// collector-side here. A close whose delivery is unknown stays requested
  /// and is re-sent after the host's next handshake.
  pub(crate) async fn close_through_host(state: &AppState, session: &SessionRow) -> Closing {
      let judged = Unattached {
          lifecycle: session.lifecycle.clone(),
          presumed_parked: session.presumed_parked,
      };
      let reachable = state.hub.is_ready(&session.host_id);
      match session.lifecycle.as_str() {
          "closed" => return Closing::Closed,
          "starting" if reachable => {
              return Closing::Answer(error(
                  StatusCode::CONFLICT,
                  "starting",
                  "the session is starting; close it once the start settles",
              ));
          }
          "active" if reachable => {}
          _ => return Closing::Unattached(judged),
      }
      let id = &session.id;
      match state.store.record_close_request(id) {
          Ok(event) => state.hub.publish(event),
          Err(err) => return Closing::Answer(internal(err)),
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      };
      match state
          .hub
          .request_for_session(&session.host_id, &request_id, &id, frame, TEARDOWN_TIMEOUT)
          .await
      {
          // `session_closed`, or a `session_parked` that overtook the close
  ```

  with:

  ```rust
      };
      match state
          .hub
          .request_for_session(&session.host_id, &request_id, id, frame, TEARDOWN_TIMEOUT)
          .await
      {
          // `session_closed`, or a `session_parked` that overtook the close
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          Ok(_) => lifecycle_response(&state, &id),
          // The host no longer has it (or went away): nothing left to stop.
          Err(RequestError::Rejected { code, .. }) if code == "not_attached" => close_unattached(&state, &id),
          Err(RequestError::NotConnected) => close_unattached(&state, &id),
          Err(err) => request_failed(err),
  ```

  with:

  ```rust
          Ok(_) => Closing::Closed,
          // The host no longer has it (or went away before the close was
          // sent): nothing left to stop, so closed collector-side, while it
          // is still as read (A4; a close request changes neither).
          Err(RequestError::Rejected { code, .. }) if code == "not_attached" => Closing::Unattached(judged),
          Err(RequestError::NotConnected) => Closing::Unattached(judged),
          Err(err) => Closing::Answer(request_failed(err)),
      }
  }

  /// Close: attached sessions are closed by their host (`session_closed`);
  /// anything else is closed immediately.
  async fn close(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
          Err(err) => return internal(err),
      };
      match close_through_host(&state, &session).await {
          Closing::Closed => lifecycle_response(&state, &id),
          Closing::Unattached(_) => close_unattached(&state, &id),
          Closing::Answer(response) => response,
      }
  }

  /// Delete (ACP core §4.10; plan 9a decision 5), behind step-up, which the
  /// route checks before this reads anything. Closed as `close` closes it,
  /// then deleted in one transaction that requires it closed, or closes it
  /// there while it is still as judged unattached (A4): 409 with the
  /// lifecycle if something moved it on meanwhile (a resume). 204.
  ///
  /// A session closed without its host (`unconfirmed`) is closed by that
  /// host when it reconciles next (decision 4). Its host may be back by the
  /// time the delete commits: a reconciliation that ran between this read and
  /// the commit left a listed active session as it was, so the judgement
  /// still held, and the delete went ahead. If the host is ready now, it is
  /// sent `close_session` here (`finish_delete`), with nobody waiting for the
  /// answer; if the commit came before it was ready, its connection finds the
  /// tombstone once it is (`ws::ready`, after `mark_ready`). A `not_attached`
  /// answer is only logged: a tombstone has nothing left to close. A
  /// connection kicked between the two sends neither; its next reconcile
  /// closes the session.
  async fn delete_session(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      let session = match state.store.find_session(&id) {
          Ok(Some(s)) => s,
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
          Err(err) => return internal(err),
      };
      let unattached = match close_through_host(&state, &session).await {
          Closing::Closed => None,
          Closing::Unattached(judged) => Some(judged),
          Closing::Answer(response) => return response,
      };
      finish_delete(&state, &session, unattached.as_ref())
  }

  /// The delete itself, once `close_through_host` has judged `session`.
  pub(crate) fn finish_delete(state: &AppState, session: &SessionRow, unattached: Option<&Unattached>) -> Response {
      let id = &session.id;
      match state.store.delete_session(id, unattached) {
          // Only `session_deleted`: what a collector-side close wrote went
          // with the session, in the same transaction.
          Ok(Deletion::Done { event, unconfirmed }) => {
              tracing::info!(session_id = %id, unconfirmed, "session deleted");
              state.hub.publish(event);
              if unconfirmed {
                  // Its host may be ready by now (it reconciled after the
                  // judgement): nobody waits for the answer, which changes
                  // nothing on a tombstone. `notify` sends only to a host that
                  // is ready.
                  state.hub.notify(
                      &session.host_id,
                      CollectorFrame::CloseSession {
                          request_id: uuid::Uuid::now_v7().to_string(),
                          session_id: id.clone(),
                      },
                  );
              }
              StatusCode::NO_CONTENT.into_response()
          }
          Ok(Deletion::Refused(lifecycle)) => error(
              StatusCode::CONFLICT,
              &lifecycle,
              format!("the session is {lifecycle} now; delete it again once that settles"),
          ),
          Ok(Deletion::NotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
          Err(err) => internal(err),
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  async fn events(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> Response {
  ```

  with:

  ```rust
  /// The session's timeline; 404 for an unknown or deleted session (plan 9a
  /// A10).
  async fn events(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> Response {
      match state.store.find_session(&id) {
          Ok(Some(_)) => {}
          Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
          Err(err) => return internal(err),
      }
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  /// events after it.
  fn replay_then_follow<E: std::fmt::Display>(
      replay: impl Stream<Item = Result<Vec<EventDto>, E>>,
      mut render: impl FnMut(&[EventDto]) -> Vec<Result<Event, Infallible>>,
      follow: impl Stream<Item = Result<Event, Infallible>>,
  ) -> impl Stream<Item = Result<Event, Infallible>> {
  ```

  with:

  ```rust
  /// events after it. The items are SSE messages, or anything a message
  /// converts into (the stream's end mark, plan 9a A10).
  fn replay_then_follow<E: std::fmt::Display, M: From<Result<Event, Infallible>>>(
      replay: impl Stream<Item = Result<Vec<EventDto>, E>>,
      mut render: impl FnMut(&[EventDto]) -> Vec<M>,
      follow: impl Stream<Item = M>,
  ) -> impl Stream<Item = M> {
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
                      vec![Ok(resync_required())]
  ```

  with:

  ```rust
                      vec![M::from(Ok(resync_required()))]
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  /// live events, until the collector shuts down or the operator's session
  /// that opened it ends (3b decision 7). 404 for a session the owner does
  /// not have (ACP core §9); the replay reads the events table a page at a
  /// time (`REPLAY_PAGE`), and a failed read sends `resync_required` and
  /// ends the stream.
  ```

  with:

  ```rust
  /// live events, until the collector shuts down, the operator's session
  /// that opened it ends (3b decision 7), or it sends the session's
  /// `session_deleted` (plan 9a A10). 404 for an unknown or deleted session
  /// (ACP core §9); the replay reads the events table a page at a time
  /// (`REPLAY_PAGE`), and a failed read sends `resync_required` and ends the
  /// stream.
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      match state.store.session(&id) {
  ```

  with:

  ```rust
      match state.store.find_session(&id) {
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
                      Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&store, &e, true)),
                      Ok(_) => None,
                      // Lagged: tell the client to refetch instead of skipping silently.
                      Err(_) => Some(vec![Ok(resync_required())]),
  ```

  with:

  ```rust
                      Ok(e) if e.session_id == session && e.event_id > last => {
                          Some(with_end_mark(sse_messages(&store, &e, true), ends_stream(&e)))
                      }
                      Ok(_) => None,
                      // Lagged: tell the client to refetch instead of skipping silently.
                      Err(_) => Some(vec![Some(Ok(resync_required()))]),
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      let stream = replay_then_follow(replay, move |page| page_messages(&store, page), follow)
  ```

  with:

  ```rust
      // The replay up to the event that ends the stream, if a page holds it,
      // and an end mark after it, which ends the stream at once, not at the
      // next message.
      let render = move |page: &[EventDto]| {
          let end = page.iter().position(ends_stream);
          let page = end.map_or(page, |end| &page[..=end]);
          with_end_mark(page_messages(&store, page), end.is_some())
      };
      let stream = replay_then_follow(replay, render, follow)
          .take_while(|message| std::future::ready(message.is_some()))
          .filter_map(std::future::ready)
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
          .into_response()
  }

  ```

  with:

  ```rust
          .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
          .into_response()
  }

  /// The event after which a session's stream ends: its tombstone (plan 9a
  /// A10), after which nothing of it is written again.
  fn ends_stream(e: &EventDto) -> bool {
      e.kind == "session_deleted"
  }

  /// `messages` as a stream's items, with the end mark (`None`) after them
  /// if `ends`.
  fn with_end_mark(messages: Vec<Result<Event, Infallible>>, ends: bool) -> Vec<Option<Result<Event, Infallible>>> {
      messages.into_iter().map(Some).chain(ends.then_some(None)).collect()
  }

  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          assert_eq!(sent.len(), 2);
      }
  }
  ```

  with:

  ```rust
          assert_eq!(sent.len(), 2);
      }
  }

  /// A delete against a host's reconciliation (plan 9a, the whole-branch
  /// review's race). Its window, between `reconcile_host`'s commit and
  /// `mark_ready`, has no await point, so these tests do not race it: they
  /// call the steps the route and the host's connection run, in each order
  /// the window allows, over a connection registered with a channel of
  /// their own.
  #[cfg(test)]
  mod delete_race_tests {
      use super::*;
      use crate::ws::{after_reconcile, ready};
      use hennery_kernel::hosts::Hosts;
      use hennery_kernel::operator::Operator;
      use hennery_proto::frames::{AttachedSession, Capabilities, SessionBody};
      use std::collections::{HashMap, HashSet};
      use tokio::sync::mpsc;

      const HOST: &str = "host-1";

      struct Fixture {
          state: AppState,
          rx: mpsc::UnboundedReceiver<CollectorFrame>,
          tx: mpsc::UnboundedSender<CollectorFrame>,
          conn_id: u64,
          _dir: tempfile::TempDir,
      }

      /// A collector's state, and `HOST` connected but not yet reconciled.
      fn fixture() -> Fixture {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let state = AppState::new(
              Store::open(&db).unwrap(),
              Hosts::open(&db).unwrap(),
              Operator::open(&db).unwrap(),
          );
          let (tx, rx) = mpsc::unbounded_channel();
          let conn_id = state
              .hub
              .register(HOST, tx.clone(), Capabilities::default())
              .unwrap()
              .conn_id;
          Fixture {
              state,
              rx,
              tx,
              conn_id,
              _dir: dir,
          }
      }

      fn session(f: &Fixture, id: &str, started: bool) -> SessionRow {
          f.state
              .store
              .create_session(id, HOST, "fake", "/tmp", "hat-1", None)
              .unwrap();
          if started {
              f.state
                  .store
                  .ingest(id, 1, &SessionBody::session_started("r0", "a1"))
                  .unwrap();
          }
          f.state.store.find_session(id).unwrap().unwrap()
      }

      fn listed(id: &str) -> Vec<AttachedSession> {
          vec![AttachedSession {
              session_id: id.into(),
              last_seq: 1,
              open_turn_id: None,
          }]
      }

      /// The route's judgement while the host is not ready: unattached, as
      /// read.
      async fn judged_unattached(f: &Fixture, session: &SessionRow) -> Unattached {
          match close_through_host(&f.state, session).await {
              Closing::Unattached(judged) => judged,
              _ => panic!("expected the session judged unattached"),
          }
      }

      /// The `close_session` frames sent to the host so far.
      fn closes(f: &mut Fixture) -> Vec<String> {
          let mut ids = Vec::new();
          while let Ok(frame) = f.rx.try_recv() {
              if let CollectorFrame::CloseSession { session_id, .. } = frame {
                  ids.push(session_id);
              }
          }
          ids
      }

      /// The delete commits after the reconciliation and before the host is
      /// ready: the route cannot reach the host, so its connection, once
      /// ready, finds the tombstone and closes it.
      #[tokio::test]
      async fn a_delete_committed_before_the_host_is_ready_is_closed_by_its_connection() {
          let mut f = fixture();
          let s = session(&f, "s1", true);
          let attached = listed("s1");
          let mut reconcile_closes = HashMap::new();
          let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
          let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
          assert!(closed.is_empty());
          let judged = judged_unattached(&f, &s).await;
          let response = finish_delete(&f.state, &s, Some(&judged));
          assert_eq!(response.status(), StatusCode::NO_CONTENT);
          assert!(
              closes(&mut f).is_empty(),
              "the host is not ready: the route sends nothing"
          );
          ready(
              &f.state,
              HOST,
              f.conn_id,
              &attached,
              &closed,
              &f.tx,
              &mut reconcile_closes,
          )
          .unwrap();
          assert_eq!(closes(&mut f), ["s1"]);
          // Tracked, so a `not_attached` answer finds its session.
          assert_eq!(reconcile_closes.values().filter(|s| *s == "s1").count(), 1);
      }

      /// The route judged the session while its host was not ready, and the
      /// delete commits once it is: the connection's check came too early,
      /// so the route closes it.
      #[tokio::test]
      async fn a_delete_committed_after_the_host_is_ready_is_closed_by_the_route() {
          let mut f = fixture();
          let s = session(&f, "s1", true);
          let attached = listed("s1");
          let mut reconcile_closes = HashMap::new();
          let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
          let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
          let judged = judged_unattached(&f, &s).await;
          ready(
              &f.state,
              HOST,
              f.conn_id,
              &attached,
              &closed,
              &f.tx,
              &mut reconcile_closes,
          )
          .unwrap();
          assert!(closes(&mut f).is_empty(), "nothing is deleted yet");
          let response = finish_delete(&f.state, &s, Some(&judged));
          assert_eq!(response.status(), StatusCode::NO_CONTENT);
          assert_eq!(closes(&mut f), ["s1"]);
      }

      /// A tombstone the reconciliation itself closes gets one
      /// `close_session`, not a second from the check after `mark_ready`.
      #[tokio::test]
      async fn a_delete_committed_before_the_reconciliation_is_closed_once() {
          let mut f = fixture();
          let s = session(&f, "s1", true);
          let attached = listed("s1");
          let judged = judged_unattached(&f, &s).await;
          assert_eq!(
              finish_delete(&f.state, &s, Some(&judged)).status(),
              StatusCode::NO_CONTENT
          );
          let mut reconcile_closes = HashMap::new();
          let done = f.state.store.reconcile_host(HOST, &attached).unwrap();
          let closed = after_reconcile(&f.state, HOST, &[], done, &f.tx, &mut reconcile_closes);
          assert_eq!(closed, HashSet::from(["s1".to_string()]));
          ready(
              &f.state,
              HOST,
              f.conn_id,
              &attached,
              &closed,
              &f.tx,
              &mut reconcile_closes,
          )
          .unwrap();
          assert_eq!(closes(&mut f), ["s1"]);
      }

      /// The route's own `starting` arm (plan 9a decision 5): refused before
      /// the store is asked, which would refuse it too.
      #[tokio::test]
      async fn a_starting_session_on_a_ready_host_is_refused_by_the_route() {
          let f = fixture();
          let s = session(&f, "s1", false);
          f.state.hub.mark_ready(HOST, f.conn_id);
          let Closing::Answer(response) = close_through_host(&f.state, &s).await else {
              panic!("expected a refusal");
          };
          assert_eq!(response.status(), StatusCode::CONFLICT);
          let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
          let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
          assert_eq!(body["code"], "starting", "{body}");
          assert_eq!(f.state.store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
      }
  }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  use std::sync::Mutex;
  ```

  with:

  ```rust
  use std::sync::atomic::{AtomicBool, Ordering};
  use std::sync::{Arc, Mutex};
  use std::time::Duration;
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// The database file, for the checkpoint after a delete, which runs on
      /// a connection of its own; an in-memory store has none.
      path: Option<PathBuf>,
  ```

  with:

  ```rust
      /// The checkpoint a delete owes (plan 9a A8); an in-memory store has
      /// none.
      checkpoints: Option<Arc<Checkpoints>>,
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  /// Fold the WAL of the database at `path` back into it and truncate it, so
  /// the pages a delete wrote leave it too (plan 9a A8): best-effort, logged.
  /// On a connection of its own, so the store's lock is not held while it
  /// waits for readers; busy if one stays, and then the next checkpoint does
  /// it.
  fn checkpoint(path: &Path) {
      let checkpointed = hennery_kernel::db::open(path)
          .and_then(|conn| Ok(conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))?));
      match checkpointed {
          Ok(0) => {}
          Ok(_) => tracing::warn!("the checkpoint after a delete was busy: the WAL keeps its pages until the next"),
          Err(err) => tracing::warn!("the checkpoint after a delete failed: {err:#}"),
  ```

  with:

  ```rust
  /// How a checkpoint after a delete is retried while a reader holds the
  /// WAL (plan 9a A8): every `retry`, `fast_retries` times, then (logged once)
  /// every `slow_retry`, until it completes. The defaults are a second, five
  /// minutes of them, then a minute.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct CheckpointPolicy {
      pub retry: Duration,
      pub fast_retries: u32,
      pub slow_retry: Duration,
  }

  impl Default for CheckpointPolicy {
      fn default() -> Self {
          Self {
              retry: Duration::from_secs(1),
              fast_retries: 300,
              slow_retry: Duration::from_secs(60),
          }
      }
  }

  /// The checkpoint a delete owes: the WAL folded back into the database and
  /// truncated, so the pages the delete wrote over leave it too (plan 9a A8).
  /// It runs on a connection of its own, so the store's lock is not held
  /// while it waits for readers. Hennery's own reads are single statements
  /// under the store's or the kernel's lock, so only a reader outside it (a
  /// `sqlite3` shell, a backup tool) can hold the WAL for long. While one
  /// does, the debt is durable: `<db>-checkpoint-owed` is written, retried by
  /// one thread until a checkpoint completes, and at the next start. A
  /// connection is opened per attempt: deletes are rare.
  struct Checkpoints {
      path: PathBuf,
      policy: Mutex<CheckpointPolicy>,
      /// A retry thread is running; a new debt joins it.
      retrying: AtomicBool,
  }

  /// How long one attempt waits for readers before it counts as busy.
  const CHECKPOINT_WAIT: Duration = Duration::from_secs(1);

  impl Checkpoints {
      fn owed_marker(&self) -> PathBuf {
          let mut name = self.path.as_os_str().to_os_string();
          name.push("-checkpoint-owed");
          PathBuf::from(name)
      }

      /// Checkpoint now; if a reader holds it up, record the debt and retry
      /// it apart until it completes.
      fn run(self: &Arc<Self>) {
          if self.once() {
              self.settle();
              return;
          }
          if let Err(err) = self.owe() {
              tracing::error!("recording a checkpoint owed failed: {err:#}");
          }
          if self.retrying.swap(true, Ordering::SeqCst) {
              return;
          }
          let this = Arc::clone(self);
          std::thread::spawn(move || {
              let policy = *this.policy.lock().expect("checkpoint policy");
              let mut attempts: u64 = 0;
              loop {
                  let pause = if attempts < u64::from(policy.fast_retries) {
                      policy.retry
                  } else {
                      if attempts == u64::from(policy.fast_retries) {
                          tracing::warn!(
                              "a reader has held the database's WAL since a delete: the deleted pages stay in the \
                               WAL until it is released; the checkpoint is retried every {:?}",
                              policy.slow_retry
                          );
                      }
                      policy.slow_retry
                  };
                  std::thread::sleep(pause);
                  attempts += 1;
                  if this.once() {
                      this.retrying.store(false, Ordering::SeqCst);
                      this.settle();
                      tracing::info!(attempts, "the checkpoint a delete owed completed");
                      return;
                  }
              }
          });
      }

      /// One `wal_checkpoint(TRUNCATE)`, waiting `CHECKPOINT_WAIT` for
      /// readers: whether it completed. A failure to open or run it is
      /// logged, and counts as not completed.
      fn once(&self) -> bool {
          let checkpointed = hennery_kernel::db::open(&self.path).and_then(|conn| {
              conn.busy_timeout(CHECKPOINT_WAIT)?;
              Ok(conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))?)
          });
          match checkpointed {
              Ok(0) => true,
              Ok(_) => false,
              Err(err) => {
                  tracing::warn!("a checkpoint after a delete failed: {err:#}");
                  false
              }
          }
      }

      /// Record the debt durably: the marker, synced, and its directory.
      fn owe(&self) -> Result<()> {
          use std::os::unix::fs::OpenOptionsExt;
          let marker = self.owed_marker();
          let file = std::fs::OpenOptions::new()
              .write(true)
              .create(true)
              .truncate(false)
              .mode(0o600)
              .open(&marker)
              .with_context(|| format!("open {}", marker.display()))?;
          file.sync_all()?;
          if let Some(dir) = marker.parent() {
              std::fs::File::open(dir)?.sync_all()?;
          }
          Ok(())
      }

      /// The debt is paid: remove the marker, if there is one.
      fn settle(&self) {
          match std::fs::remove_file(self.owed_marker()) {
              Ok(()) => {}
              Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
              Err(err) => tracing::warn!("removing the checkpoint-owed marker failed: {err}"),
          }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          hennery_kernel::db::migrate(&mut conn, MIGRATIONS)?;
          Ok(Self {
  ```

  with:

  ```rust
          hennery_kernel::db::migrate(&mut conn, MIGRATIONS)?;
          let checkpoints = path.map(|path| {
              Arc::new(Checkpoints {
                  path,
                  policy: Mutex::new(CheckpointPolicy::default()),
                  retrying: AtomicBool::new(false),
              })
          });
          // A checkpoint owed from before a restart is paid first (A8).
          if let Some(checkpoints) = &checkpoints
              && checkpoints.owed_marker().exists()
          {
              checkpoints.run();
          }
          Ok(Self {
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              path,
          })
  ```

  with:

  ```rust
              checkpoints,
          })
      }

      /// How a checkpoint a reader holds up is retried (plan 9a A8), for the
      /// next one that is.
      pub fn set_checkpoint_policy(&self, policy: CheckpointPolicy) {
          if let Some(checkpoints) = &self.checkpoints {
              *checkpoints.policy.lock().expect("checkpoint policy") = policy;
          }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          Ok(Some(row))
      }
  ```

  with:

  ```rust
          Ok(Some(row))
      }

      /// Which of `ids` are tombstones of `host_id`'s (plan 9a decision 4):
      /// what a host's connection, once ready, still has to close of the
      /// sessions its `hello` listed, if a delete committed after its
      /// reconciliation.
      pub(crate) fn tombstones_of(&self, host_id: &str, ids: &[&str]) -> Result<Vec<String>> {
          let conn = self.conn();
          let mut stmt = conn.prepare(
              "SELECT 1 FROM sessions WHERE id = ?1 AND host_id = ?2 AND lifecycle = 'deleted' AND owner_id = ?3",
          )?;
          let mut deleted = Vec::new();
          for id in ids {
              if stmt.exists(params![id, host_id, self.owner])? {
                  deleted.push(id.to_string());
              }
          }
          Ok(deleted)
      }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          if let Some(path) = self.path.as_deref() {
              checkpoint(path);
  ```

  with:

  ```rust
          if let Some(checkpoints) = &self.checkpoints {
              checkpoints.run();
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
  use crate::store::{Edge, Ingested, PushEdge, Store};
  ```

  with:

  ```rust
  use crate::store::{Edge, Ingested, PushEdge, Reconciliation, Store};
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
  use hennery_proto::frames::{CollectorFrame, HostFrame, SessionBody};
  use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION, protocol_major};
  use std::collections::{BTreeMap, HashMap};
  ```

  with:

  ```rust
  use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame, SessionBody};
  use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION, protocol_major};
  use std::collections::{BTreeMap, HashMap, HashSet};
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                          for event in done.events {
                              state.hub.publish(event);
                          }
                          // Only a reconciled connection's roots are stored
                          // (decision 7), before the host is listed as
                          // connected with them.
                          if let Err(err) = state.hosts.record_workspace_roots(&host_id, &workspace_roots) {
                              tracing::warn!(%host_id, error = %err, "recording the host's workspace roots failed");
                          }
                          for session_id in done.close {
                              let request_id = uuid::Uuid::now_v7().to_string();
                              reconcile_closes.insert(request_id.clone(), session_id.clone());
                              let _ = tx.send(CollectorFrame::CloseSession { request_id, session_id });
                          }
                          reconciled = true;
                          state.hub.mark_ready(&host_id, conn_id);
                          // The answer queue (ACP core §5.1 step 4), after the
                          // reconciliation cancelled what a restart lost, and
                          // after `mark_ready`: an answer submitted meanwhile
                          // is either read here or sent by its own handler
                          // (or both; the host dedupes by pending id).
                          match state.store.answers_to_send(&host_id) {
                              Ok(answers) => {
                                  for frame in answers {
                                      let _ = tx.send(frame);
                                  }
                              }
                              Err(err) => {
                                  tracing::error!(%host_id, error = %err, "reading the answer queue failed; dropping connection");
                                  break;
                              }
  ```

  with:

  ```rust
                          let closed =
                              after_reconcile(&state, &host_id, &workspace_roots, done, &tx, &mut reconcile_closes);
                          reconciled = true;
                          if let Err(err) = ready(
                              &state,
                              &host_id,
                              conn_id,
                              &attached_sessions,
                              &closed,
                              &tx,
                              &mut reconcile_closes,
                          ) {
                              tracing::error!(%host_id, error = %err, "reading the answer queue failed; dropping connection");
                              break;
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
      crate::offline::after_disconnect(&state, host_id, conn_id);
  }
  ```

  with:

  ```rust
      crate::offline::after_disconnect(&state, host_id, conn_id);
  }

  /// What a committed reconciliation (ACP core §5.1 step 4) sends and records
  /// before the host is ready: its events, the host's workspace roots, and a
  /// `close_session` for each attached session the operator closed or
  /// deleted. The sessions sent one.
  pub(crate) fn after_reconcile(
      state: &AppState,
      host_id: &str,
      workspace_roots: &[String],
      done: Reconciliation,
      tx: &mpsc::UnboundedSender<CollectorFrame>,
      reconcile_closes: &mut HashMap<String, String>,
  ) -> HashSet<String> {
      for event in done.events {
          state.hub.publish(event);
      }
      // Only a reconciled connection's roots are stored (decision 7), before
      // the host is listed as connected with them.
      if let Err(err) = state.hosts.record_workspace_roots(host_id, workspace_roots) {
          tracing::warn!(%host_id, error = %err, "recording the host's workspace roots failed");
      }
      let mut closed = HashSet::new();
      for session_id in done.close {
          send_reconcile_close(tx, reconcile_closes, &session_id);
          closed.insert(session_id);
      }
      closed
  }

  /// Mark the host ready, then send what waited for that. An error means
  /// the answer queue could not be read: the caller drops the connection.
  pub(crate) fn ready(
      state: &AppState,
      host_id: &str,
      conn_id: u64,
      attached: &[AttachedSession],
      closed: &HashSet<String>,
      tx: &mpsc::UnboundedSender<CollectorFrame>,
      reconcile_closes: &mut HashMap<String, String>,
  ) -> anyhow::Result<()> {
      // The read below must stay after `mark_ready`: moved before it, a
      // delete committed between the two is closed by neither side, and no
      // test would notice (plan 9a's whole-branch review).
      state.hub.mark_ready(host_id, conn_id);
      // A delete that judged a listed session unattached (this host not ready
      // yet) and committed after the reconciliation: nothing above closed its
      // adapter. Read after `mark_ready`, so a delete committed before this
      // read is found here, and one committed after it finds the host ready
      // and sends the close itself (`api::finish_delete`). Both may: the
      // second is answered `not_attached`, which changes nothing on a
      // tombstone (plan 9a decision 4). If the read fails, the next
      // reconnect's reconciliation closes it.
      let listed: Vec<&str> = attached
          .iter()
          .map(|a| a.session_id.as_str())
          .filter(|id| !closed.contains(*id))
          .collect();
      match state.store.tombstones_of(host_id, &listed) {
          Ok(deleted) => {
              for session_id in deleted {
                  send_reconcile_close(tx, reconcile_closes, &session_id);
              }
          }
          Err(err) => {
              tracing::warn!(%host_id, error = %err, "checking for sessions deleted during reconciliation failed");
          }
      }
      // The answer queue (ACP core §5.1 step 4), after the reconciliation
      // cancelled what a restart lost, and after `mark_ready`: an answer
      // submitted meanwhile is either read here or sent by its own handler
      // (or both; the host dedupes by pending id).
      for frame in state.store.answers_to_send(host_id)? {
          let _ = tx.send(frame);
      }
      Ok(())
  }

  /// A `close_session` the connection itself sends, tracked by its request
  /// id (see `reconcile_closes` in `serve`).
  fn send_reconcile_close(
      tx: &mpsc::UnboundedSender<CollectorFrame>,
      reconcile_closes: &mut HashMap<String, String>,
      session_id: &str,
  ) {
      let request_id = uuid::Uuid::now_v7().to_string();
      reconcile_closes.insert(request_id.clone(), session_id.to_string());
      let _ = tx.send(CollectorFrame::CloseSession {
          request_id,
          session_id: session_id.to_string(),
      });
  }
  ```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-sessions --locked --lib api` and `cargo test -p hennery-testkit --locked --test reconcile --test images --test step_up --test auth`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

- Each of these, removed, makes a test fail:
  - the `session_deleted` publish;
  - the stream's end after `session_deleted`;
  - the step-up layer (removed, or moved onto the whole path);
  - the 404 checks on `events` and on the stream;
  - the store delete call;
  - the 503 answer;
  - the `not_attached` branch;
  - the hand-off of the state as read.
- In the reconnect race:
  - removing the route's close after an unconfirmed delete fails `a_delete_committed_after_the_host_is_ready_is_closed_by_the_route`;
  - removing ws.rs's close after `mark_ready` fails `a_delete_committed_before_the_host_is_ready_is_closed_by_its_connection`;
  - removing its `!closed.contains` filter fails `a_delete_committed_before_the_reconciliation_is_closed_once`;
  - removing the route's `starting` arm fails `a_starting_session_on_a_ready_host_is_refused_by_the_route`.
- In the checkpoint debt: not spawning the retry thread fails `a_checkpoint_a_reader_held_up_is_retried_once_it_is_gone` and the past-deadline test; not recording the debt fails the past-deadline test; stopping after the fast retries fails it too; not removing the record on success fails it; not paying a record at open fails `a_checkpoint_owed_at_a_restart_is_paid_when_the_store_opens`.
- Argued, not tested: the re-check running after `mark_ready`, and `serve` calling the two steps with no await between them.

- [ ] **Step 6: The full checks**

Expected: all pass; **1000 tests** on `4031ba0`, 1064 on `fc00485`.

- [ ] **Step 7: Commit**

```bash
git add crates
git commit -m "feat(sessions): DELETE /api/sessions/{id} deletes a session, behind step-up"
```

## After this plan

**What the frontend must do (plan 4):**
- **"Delete session"** in the header menu, with a confirmation. On 403 `step_up_required`, step up and retry.
- **The answers:**
  - 204: drop the session from the list and close its view;
  - 404: it is gone already;
  - 409 `starting`: wait for the start to settle;
  - 409 with another lifecycle as its code (`active`, `parked`, …): the session moved meanwhile; refetch and ask again;
  - 503 `delivery_unknown`: nothing was deleted and the close stays requested; retry later.
- **The stream** of a deleted session sends `session_deleted` and ends. A reconnect gets 404.
- **The browser cache:** images are served `immutable`. A deleted image can stay in the cache until the logout change (6a's O1) clears it.

**Obligations this plan hands on:**
- **9b, the sweep:** files left by a crash between the delete's commit and its removal; a `.tmp` from a crash; images whose rows lost a race. It reuses `shared_files::hash_named_by_any_owner` and the checkpoint connection.
- **9c, the purge:** deletes each of a hat's sessions with `delete_session`, one transaction each, and refuses a frozen hat in `create_session`, `request_resume` and `reassign_hat`.
- **The gateway (plan 8):** revoke the session's tokens where `delete_session` commits. The comment `plan 8: revoke the session's gateway tokens here` marks the place.
- **The list stream** (`GET /api/stream/sessions`): `session_deleted` becomes `session_removed`, and a tombstone is never upserted.
- **Later migrations** that `UPDATE sessions` must exclude tombstones, or the trigger aborts them (the comment beside the triggers says so).
- **Backups** taken before a delete keep its data; the operator's docs must say so (A8).
- **9d, the host's transcript** (operator delegated, parent decided 2026-10-02): the delete scrubs `agent_session_id` and `cwd`, which 9d needs to find the agent's files. So 9d's migration adds a record of what is still to be removed on each host, written in `delete_session`'s transaction before the scrub and dropped once the host confirms. Sessions deleted between 9a and 9d have their files left; nothing is released in between.

**Not tested here:**
- **A power loss** between the commit and the file removal (9b's sweep is the answer).
- **`secure_delete` on Linux:** the raw-bytes test ran on macOS; ubuntu CI runs it too.
- **The wiring of `after_reconcile` and `ready` in `serve`:** covered by the integration tests, not ordered by them.

**Spec amendments** (written back by this plan's docs commit):
- ACP core §4.10: decisions 1, 4, 5, 6, 8 and 9;
- ACP core §8: `turn_attachments`, the tombstone, the triggers, and `session_deleted` written;
- ACP core §9: the `DELETE` row's answers, the 404s and the stream end, and "built so far";
- kernel §1: `secure_delete`;
- umbrella §6.10: what a delete leaves (the tombstone, earlier backups, the browser cache).

Then, in order:
- **(9b) The orphan sweep**
- **(9c) The per-hat purge**
- **(4) Frontend shell**

---

_Generated with Claude AI — please review before distribution._
