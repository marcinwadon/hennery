# Delete and purge (plan 9d-i): the agent's own transcript on its host, Claude Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** A delete or a per-hat purge also removes, best effort, the agent's own transcript of the session on its host. What could not be removed is reported with the result, and the removal is retried when the host reconnects.
- **At the start:** the host records where each session's agent writes (its home), and the collector stores it.
- **At the delete:** in the same transaction, before 9a's scrub, the delete writes a record of what is still to be removed on the host (`host_forgets`).
- **The protocol:**
  - the collector sends `forget_session` to a host that announces the capability `forget_session`;
  - the host answers `session_forgotten`, with what it removed and what it left, as kinds and counts, never paths.
- **On the host, for Claude:**
  - the adapter's own `session/delete`;
  - then the transcript family under `projects/*/` and the exact `file-history/<id>/`, `session-env/<id>/`, `tasks/<id>/` and `debug/<id>.txt`;
  - removed through directory descriptors, with no link ever followed;
  - checked afterwards.
- **Codex (9d-ii):** its home is recorded here, and its forget answers `unsupported_agent`, retryable, until 9d-ii.
- **Known limitation, stated in every Claude result:** transcripts started after a context clear inside the agent are not removed.

This is plan 9d's first part, in two PRs:
- **9d-i-a**, Tasks 1–2: the recording, the record, the protocol and the results;
- **9d-i-b**, Tasks 3–4: the Claude path on the host.

9d-ii (Codex, `codex app-server` and `thread/delete`) follows.

**Decided by:** the operator delegated it, and the parent decided on 2026-10-02 (see "Decisions").

**Architecture:**
- **Host** (`hennery-host`):
  - **`agent_home.rs`:**
    - resolves the roots from the adapter's environment: `CLAUDE_CONFIG_DIR` or `$HOME/.claude`, and `CODEX_HOME` (plus `CODEX_SQLITE_HOME`) or `$HOME/.codex`;
    - keeps the host's own registry of `(agent, agent_session_id, roots)`, written durably before `session_started` (B1).
  - **`forget.rs`:**
    - validates the id, then the registered root;
    - checks the root's and the kind directories' owner and mode, where only the user's private group may share write access;
    - runs the adapter's `session/delete` through `Adapter::spawn`'s hygiene;
    - removes the exact names;
    - decides the outcome by checking afterwards (B4).
  - **`walk.rs`:**
    - a descriptor walk (`openat` `O_NOFOLLOW|O_DIRECTORY`, `fdopendir` on a dup, `fstatat(AT_SYMLINK_NOFOLLOW)`, `unlinkat`), never `d_type` alone;
    - bounded in depth and in the descriptors it holds;
    - stops at a mount point and at its deadline.
  - **`connection.rs`:**
    - the `forget_session` arm;
    - the live-actor check and the attach marker, under the sessions lock (B7).
- **Collector** (`hennery-sessions`):
  - **Migration 14:** `sessions.agent_home` and `host_forgets`.
  - **`forget.rs`:**
    - sends right after a delete (waiting at most 30 s), after `ws::ready`, and after a session's `session_closed`;
    - one attempt per record in flight, with a rerun on request;
    - decides when a record is done or final;
    - re-checks `shared` before each send.
  - **The routes:**
    - `DELETE /api/sessions/{id}` → 200 `DeleteResult {host_transcript}`;
    - `GET /api/settings/host-removals`;
    - `DELETE /api/settings/host-removals/{id}` (step-up);
    - `PurgeResult.host_transcripts`.
- **Wire:**
  - the frames `forget_session` and `session_forgotten`;
  - `agent_home` in `session_started`;
  - the `forget_session` capability;
  - `ForgetKind` and `ForgetReason`;
  - `DeleteResult`, `TranscriptRemoval`, `HostRemovalItem` and `HostTranscripts`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8, `libc` (already a direct dependency of `hennery-host`). No new crates.

**Spec:**
- **Umbrella §6.10:** a delete removes the session's data.
- **ACP core §4.10:** amended by this plan with the transcript on the host.
- **The parent's decision, operator delegated, 2026-10-02:**
  - "a delete or a per-hat purge also removes the agent's own transcript on the host, best effort … only that session's files inside the agent's known session directory … never follow symlinks … report back what couldn't be removed … and retry when the host reconnects";
  - for Claude, also the exact `file-history/<id>/`, `session-env/<id>/`, `tasks/<id>/` and `debug/<id>.txt` under the recorded `CLAUDE_CONFIG_DIR`, with the id validated first.

It builds on plans 9a (`delete_session`, the scrub), 9c (the purge, `delete_rows`) and 7b (the managed adapters). The evidence was checked against claude-agent-acp 0.81.0 (SDK 0.3.280), codex-acp 1.13.0 and Codex 0.155.1. Every anchor was taken from `main` at `953d0e9`.

**Status:** executed 2026-10-02 (see "Execution status").
- The security review of the design (opus, on the maintainer's behalf) answered "approve after amendments" (B1–B9, O10–O12), then re-confirmed it with binding notes on the descriptor walk (R1–R4).
- The security review of the code answered "approve after amendments" (13 items), then re-confirmed it with notes.

**How the code blocks were made and checked:**
- Every block below was generated from the reviewed commits, as diffs from `953d0e9`.
- The plan was replayed from its own text onto `953d0e9`, task by task, and the tree matched each task's commit byte for byte (`replay.py`, every block applied).

## Execution status (2026-10-02)

**Executed** on `main` at `953d0e9`:
- One opus implementer, with the design review and its re-confirmation first.
- A security review of the code, its fixes, and its re-confirmation.
- A rebase onto `953d0e9`, where 9a, 9b and 9c are merged. The two parts' commits were re-ordered there, so 9d-i-a ends where 9d-i-b starts.
- A scratch draft PR ran ubuntu early (the fleet's "run Linux early" rule).

| Area | As built | Why |
|---|---|---|
| The split | 9d-i-a: the host answers `unsupported_agent` for every agent. 9d-i-b: the Claude path. | One reviewable PR each. |
| Readings the decisions left open | `what` is a kind and a count, never a path. The notes, including the context-clear limitation and the other residue, are the collector's own, so a Claude forget can be complete. Added a `state` column (`pending` / `final`), the reasons `in_progress`, `invalid_id`, `host_revoked` and `timed_out`, and the whole-forget kind `session`. Registry entries are kept after a complete forget, so a retry after a lost answer gets the same answer. | Confirmed by the code review. |
| Code review, binding | A listing that fails is `io_error`, retryable, in both passes. The depth bound is judged on the descriptor that `descend` opened. A second forget of the same agent session is `in_progress`. The walk stops at its deadline (`timed_out`). Symlinked project directories are reported, never followed. Group-writable is allowed only for the user's private group (`safe_mode`, through `getpwuid_r` and `getgrgid_r`). | B3 as first written refused every user whose umask is 002, the Ubuntu default with private groups. The rest are B4, R2 and decision 8. |
| Code review, optional, all taken | Mount-stop unit tests. A swapped top entry is reported as a link, not unlinked. The root is checked by its device and inode. `projects/` is found by its kind. A rerun on request. Final records keep no roots. `shared` is re-checked before each send. | Defence in depth, and liveness. |
| Re-confirmation notes | The rerun test is bounded to `1..=2` attempts. The account lookup runs in the forget's blocking task, not on the connection loop. | A slow LDAP or sssd lookup must not block a connection. |
| The 9c hand-off | A purge writes each session's forget records through 9c's `delete_rows`, before the scrub, and reports `host_transcripts {removed, partial, pending}` with the pending ids, within one 30 s wait for the whole purge. | Decision 7. |
| The per-outcome rule | Every outcome of `ForgetReason`, `ForgetKind`, the outcome itself, the `TranscriptRemoval` states, the walk's `Stop` and `safe_mode` has its own test and probe. `codex_database_copies` waits for 9d-ii, where it is produced. | Fleet rule. |

Checks:
- After each part the five checks passed: 1274 tests at 9d-i-a and 1300 at 9d-i-b on `e4e2ca3`, from 1243; after the rebase onto `953d0e9`, 1378 at 9d-i-b, from 1321.
- The forget binaries passed with 4 copies in parallel and under `umask 002`.
- The run was macOS. The scratch PR ran ubuntu (green after one fix: a test assumed a group-writable root is refused, true for macOS's shared `staff` group but not for ubuntu's private group, which the reviewed rule allows; the case now uses an other-writable root).

## Scope

**4 tasks:**
1. Recording the home, the record, the protocol and the results (9d-i-a).
2. The purge's forgets and counts (9d-i-a).
3. The Claude path on the host (9d-i-b).
4. Its hardening after the review, and the per-outcome tests (9d-i-b).

**Out of scope:**
- **Codex (9d-ii):** `codex app-server` with `thread/delete`; the fallback (archive, then the rollout files); `codex_database_copies`; the pin-bump checklist.
- **Sessions from before 9d:** they have no recorded home and are reported `no_recorded_home` (decision 11).
- **The composed per-hat homes (plan 8):** their recorded root is the composed home.

## Decisions this plan makes where the spec is silent

The design is in plan 9's [design record](2026-10-21-delete-and-purge-design-record.md) §2, and these are its decisions as built. The security reviews confirmed them on the maintainer's behalf.

1. **The home is recorded at start** (decision 1, B1, B8).
   - The host resolves the agent's roots from the adapter's environment.
   - It writes every distinct `(agent_session_id, roots)` pair to its own registry, synced, before `session_started`, at most 8.
   - The collector stores them, after checking their shape: absolute, bounded, no NUL.
2. **The record** (decisions 2 and 3, B8).
   - `host_forgets` is written in the delete's own transaction, before the scrub, for each recorded pair. The scrub then clears `agent_home`.
   - No record is written for an agent session that another kept session still refers to (`shared`).
   - A record never holds a cwd or content.
3. **The protocol** (decisions 4–6, B2, B7, O10).
   - `forget_session` is gated by the capability.
   - The reply carries kinds and counts, and fixed reason codes chosen by the host.
   - One attempt per record is in flight.
   - **When a record is done:** when the outcome is complete, or when every remaining item has `retry: false`.
   - **When a record is final:** `invalid`, `unknown_to_host`, `shared`, a revoked host, or a dismissal (which needs step-up).
4. **The results** (decision 7).
   - `DeleteResult {host_transcript: {state: removed|partial|pending|none, remaining[{what, reason}], notes[]}}`;
   - `PurgeResult.host_transcripts`.
5. **The Claude path** (decision 8, B3–B6, B9, R1–R4).
   - The steps, in order:
     - validate the id (a lowercase UUID);
     - check the root: registered by exact bytes, its owner and mode, its device and inode;
     - check the kind directories by descriptor;
     - run the adapter's `session/delete`, with a not-found counted as success;
     - remove the transcript family and the exact names through the walk;
     - check afterwards that nothing named remains.
   - The adapter's own delete is outside the no-follow guarantee (B3), so a symlinked `projects/*` entry is reported.
6. **Who may write** (the code review, (a)).
   - The owner must be the euid.
   - Other-writable is refused.
   - Group-writable is allowed only for the user's private group: the user's primary gid, the user's own name, and no other member.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`.
- The five checks pass.
- No new crates. **Wire types change:** regenerate them.
- Every collector query names the owner. The host's registry is in the owner audit's `EXEMPT` list (the host's own database).
- No production test hooks. The walk's swap test uses the host crate's existing `test-hooks` feature.
- Nothing is macOS-only. The walk's tests run on ubuntu CI.
- Commits are unsigned (fleet rule), with the gmail identity.

## Review Focus

1. **Only the session's own entries go, never through a link.**
   - Tests: `a_forget_removes_exactly_the_sessions_entries`, `a_symlink_at_a_named_entry_is_reported_and_never_followed`, `a_symlink_inside_a_removed_directory_is_unlinked_not_followed`, `a_directory_swapped_for_a_symlink_after_listing_is_not_followed`, `a_symlinked_project_directory_is_not_walked`.
2. **Never another root, another session or another user's directory.**
   - Tests: `a_root_the_host_never_registered_is_refused`, `an_invalid_agent_session_id_is_refused_for_good`, `an_agent_session_another_session_still_uses_is_left_shared`, `a_kind_directory_writable_by_others_is_skipped`, `a_group_writable_root_passes_only_for_a_private_group`.
3. **Never while it runs.**
   - Tests: `a_forget_runs_only_for_a_registered_home_with_no_live_actor`, `an_attach_waits_out_a_forget_of_the_same_agent_session`.
4. **What is left is reported and retried.**
   - Tests: `a_delete_while_the_host_is_away_is_pending_and_retried_at_its_return`, `a_project_directory_that_cannot_be_listed_is_left_for_a_retry`, `a_forget_answered_attached_goes_again_after_the_sessions_session_closed`, `a_host_removal_is_listed_until_it_is_dismissed`.
5. **End to end.**
   - Tests: `a_delete_over_http_removes_the_claude_transcript_on_its_host`, `a_purge_forgets_its_sessions_on_their_hosts_and_counts_them`.

**Reading the steps:** as in plan 9a.

---

### Task 1: The home, the record, the protocol and the results (9d-i-a)

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-proto/tests/frames.rs`, replace:

  ```rust
      assert!(frame.probe_capability().is_err());
  }
  ```

  with:

  ```rust
      assert!(frame.probe_capability().is_err());
  }

  // Plan 9d: the agent's own transcript on its host.

  #[test]
  fn forget_frames_use_the_spec_field_names_and_name_kinds_never_paths() {
      use hennery_proto::frames::{AgentHome, ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat};
      let forget = CollectorFrame::ForgetSession {
          request_id: "r".into(),
          agent: "claude".into(),
          agent_session_id: "a1".into(),
          agent_home: AgentHome {
              root: "/h/.claude".into(),
              sqlite_root: None,
          },
      };
      let expected = json!({
          "type": "forget_session", "request_id": "r", "agent": "claude", "agent_session_id": "a1",
          "agent_home": {"root": "/h/.claude"}
      });
      assert_eq!(serde_json::to_value(&forget).unwrap(), expected);
      assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), forget);
      assert_eq!(
          forget.probe_capability(),
          Ok(Some(hennery_proto::frames::Capability::ForgetSession))
      );

      let answer = HostFrame::SessionForgotten {
          request_id: "r".into(),
          outcome: ForgetOutcome::Partial,
          removed: vec![ForgetWhat {
              kind: ForgetKind::Transcript,
              count: 2,
          }],
          remaining: vec![ForgetRemaining {
              what: ForgetWhat {
                  kind: ForgetKind::FileHistory,
                  count: 1,
              },
              reason: ForgetReason::Symlink,
              retry: false,
          }],
      };
      let expected = json!({
          "type": "session_forgotten", "request_id": "r", "outcome": "partial",
          "removed": [{"kind": "transcript", "count": 2}],
          "remaining": [{"what": {"kind": "file_history", "count": 1}, "reason": "symlink", "retry": false}]
      });
      assert_eq!(serde_json::to_value(&answer).unwrap(), expected);
      assert_eq!(serde_json::from_value::<HostFrame>(expected).unwrap(), answer);
      assert_eq!(answer.probe_request_id(), Some("r"));
      // A reason is a closed set: free text from a host does not parse.
      let free = json!({
          "type": "session_forgotten", "request_id": "r", "outcome": "partial", "removed": [],
          "remaining": [{"what": {"kind": "debug", "count": 1}, "reason": "/home/me/.claude/debug", "retry": true}]
      });
      assert!(serde_json::from_value::<HostFrame>(free).is_err());
  }

  #[test]
  fn session_started_carries_the_agent_home_only_when_there_is_one() {
      use hennery_proto::frames::AgentHome;
      let plain = SessionBody::session_started("r", "a1");
      assert!(serde_json::to_value(&plain).unwrap().get("agent_home").is_none());
      let with = json!({
          "kind": "session_started", "request_id": "r", "agent_session_id": "a1",
          "agent_home": {"root": "/h/.codex", "sqlite_root": "/h/db"}
      });
      let SessionBody::SessionStarted { agent_home, .. } = serde_json::from_value(with).unwrap() else {
          panic!("not a session_started");
      };
      assert_eq!(
          agent_home,
          Some(AgentHome {
              root: "/h/.codex".into(),
              sqlite_root: Some("/h/db".into())
          })
      );
  }

  #[test]
  fn an_agent_home_is_well_formed_only_absolute_bounded_and_without_nul() {
      use hennery_proto::frames::{AGENT_HOME_MAX_BYTES, AgentHome};
      let home = |root: &str, sqlite: Option<&str>| AgentHome {
          root: root.into(),
          sqlite_root: sqlite.map(Into::into),
      };
      assert!(home("/h/.claude", None).is_well_formed());
      assert!(home("/h/.codex", Some("/db")).is_well_formed());
      assert!(!home("h/.claude", None).is_well_formed());
      assert!(!home("", None).is_well_formed());
      assert!(!home("/h/\0x", None).is_well_formed());
      assert!(!home("/h", Some("db")).is_well_formed());
      assert!(!home(&format!("/{}", "a".repeat(AGENT_HOME_MAX_BYTES)), None).is_well_formed());
      assert!(home(&format!("/{}", "a".repeat(AGENT_HOME_MAX_BYTES - 1)), None).is_well_formed());
  }
  ```

  Create `crates/hennery-sessions/tests/forget_store.rs`:

  ```rust
  //! The agent's own transcript on its host, in the store (plan 9d decisions
  //! 1–3, 6, B8, O10): the homes a session records, the records a delete
  //! writes, and what becomes of them.

  use hennery_proto::frames::{AgentHome, ForgetReason, SessionBody};
  use hennery_proto::rest::{HostRemovalState, RemovalPending, RemovalState, TranscriptRemoval};
  use hennery_sessions::store::{Deletion, HostForgets, MAX_AGENT_HOMES, Store};
  use rusqlite::Connection;
  use std::path::{Path, PathBuf};

  fn file_store(dir: &Path) -> (Store, PathBuf) {
      let db = dir.join("hennery.db");
      (Store::open(&db).unwrap(), db)
  }

  fn home(root: &str) -> AgentHome {
      AgentHome {
          root: root.into(),
          sqlite_root: None,
      }
  }

  fn started_with(id: &str, agent_session_id: &str, agent_home: Option<AgentHome>) -> SessionBody {
      SessionBody::SessionStarted {
          request_id: format!("r-{id}"),
          agent_session_id: agent_session_id.into(),
          indexed: Default::default(),
          agent_home,
      }
  }

  /// `id` on `h1`, started by `agent` with `agent_session_id` in `root`.
  fn started(store: &Store, id: &str, agent: &str, agent_session_id: &str, root: Option<&str>) {
      store
          .create_session(id, "h1", agent, "/srv/app", "hat-1", None)
          .unwrap();
      store
          .ingest(id, 1, &started_with(id, agent_session_id, root.map(home)))
          .unwrap();
  }

  fn recorded(conn: &Connection, id: &str) -> Option<serde_json::Value> {
      conn.query_row("SELECT agent_home FROM sessions WHERE id = ?1", [id], |r| {
          r.get::<_, Option<String>>(0)
      })
      .unwrap()
      .map(|raw| serde_json::from_str(&raw).unwrap())
  }

  /// Close and delete `id`: what it left for its host.
  fn delete(store: &Store, id: &str) -> HostForgets {
      store.close_now(id).unwrap();
      match store.delete_session(id, None).unwrap() {
          Deletion::Done { forgets, .. } => *forgets,
          other => panic!("not deleted: {other:?}"),
      }
  }

  fn pending_result(why: RemovalPending) -> TranscriptRemoval {
      TranscriptRemoval {
          state: RemovalState::Pending,
          pending: Some(why),
          remaining: Vec::new(),
          notes: Vec::new(),
      }
  }

  /// Decision 1, B8: each distinct (agent session id, roots) a start or
  /// resume reports is recorded, once, at most `MAX_AGENT_HOMES`; one that
  /// fails the shape check is not; a re-sent `session_started` that changes
  /// nothing records nothing.
  #[test]
  fn a_session_records_each_distinct_agent_home_it_reports_up_to_the_cap() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let conn = Connection::open(&db).unwrap();
      started(&store, "s1", "claude", "a1", Some("/h/.claude"));
      assert_eq!(
          recorded(&conn, "s1"),
          Some(serde_json::json!([{"agent_session_id": "a1", "root": "/h/.claude"}]))
      );
      // A restart's re-sent `session_started` for the active session: not
      // applied, not recorded.
      store
          .ingest("s1", 2, &started_with("s1", "a1", Some(home("/elsewhere"))))
          .unwrap();
      assert_eq!(recorded(&conn, "s1").unwrap().as_array().unwrap().len(), 1);

      let mut seq = 2;
      let mut resume = |root: &str, id: &str| {
          store.mark_failed("s1", "x").unwrap();
          seq += 1;
          store
              .ingest("s1", seq, &started_with("s1", id, Some(home(root))))
              .unwrap();
      };
      resume("/h/.claude", "a1");
      resume("relative", "a1");
      resume("/h/\0nul", "a1");
      resume("/h/.claude", "a2");
      assert_eq!(
          recorded(&conn, "s1"),
          Some(serde_json::json!([
              {"agent_session_id": "a1", "root": "/h/.claude"},
              {"agent_session_id": "a2", "root": "/h/.claude"}
          ]))
      );
      for n in 0..MAX_AGENT_HOMES + 2 {
          resume(&format!("/root-{n}"), "a1");
      }
      assert_eq!(
          recorded(&conn, "s1").unwrap().as_array().unwrap().len(),
          MAX_AGENT_HOMES
      );
  }

  /// Decision 2, B8: a delete writes one record per pair, in its own
  /// transaction, with the agent's ids and roots only, and clears the
  /// session's home. An agent session id with no home (from before 9d) gets
  /// a final record (decision 11); a session with no agent record gets none.
  #[test]
  fn a_delete_writes_one_record_per_pair_and_keeps_no_content() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let conn = Connection::open(&db).unwrap();
      started(&store, "s1", "claude", "a1", Some("/h/.claude"));
      store.mark_failed("s1", "x").unwrap();
      store
          .ingest("s1", 2, &started_with("s1", "a2", Some(home("/h/other"))))
          .unwrap();
      let forgets = delete(&store, "s1");
      assert!(forgets.had_agent_record);
      assert_eq!(forgets.agent, "claude");
      let pairs: Vec<(String, Option<String>, HostRemovalState)> = forgets
          .records
          .iter()
          .map(|r| {
              (
                  r.agent_session_id.clone(),
                  r.agent_home.as_ref().map(|h| h.root.clone()),
                  r.state,
              )
          })
          .collect();
      assert_eq!(
          pairs,
          [
              ("a1".into(), Some("/h/.claude".into()), HostRemovalState::Pending),
              ("a2".into(), Some("/h/other".into()), HostRemovalState::Pending),
          ]
      );
      assert_eq!(recorded(&conn, "s1"), None);
      assert_eq!(store.forgets_to_send("h1").unwrap(), forgets.records);
      assert_eq!(store.forgets_of_session("s1").unwrap(), forgets.records);
      // The rows hold no cwd, title or content of the session.
      let dump: String = conn
          .query_row(
              "SELECT group_concat(id || host_id || session_id || hat_id || agent || agent_session_id
                       || COALESCE(agent_home, '') || created_at, '|') FROM host_forgets",
              [],
              |r| r.get(0),
          )
          .unwrap();
      assert!(!dump.contains("/srv/app"), "{dump}");

      // From before 9d: an agent session id, no home. Final from the start.
      store
          .create_session("s2", "h1", "claude", "/srv/b", "hat-1", None)
          .unwrap();
      store.ingest("s2", 1, &SessionBody::session_started("r", "b1")).unwrap();
      let forgets = delete(&store, "s2");
      let [record] = forgets.records.as_slice() else {
          panic!("{forgets:?}");
      };
      assert_eq!(
          (record.state, record.agent_home.as_ref()),
          (HostRemovalState::Final, None)
      );
      assert_eq!(
          record.last_result.as_ref().unwrap().remaining[0].reason,
          ForgetReason::NoRecordedHome
      );
      assert!(
          store
              .forgets_to_send("h1")
              .unwrap()
              .iter()
              .all(|r| r.session_id == "s1")
      );

      // Never started: nothing on the host.
      store
          .create_session("s3", "h1", "claude", "/srv/c", "hat-1", None)
          .unwrap();
      let forgets = delete(&store, "s3");
      assert!(!forgets.had_agent_record && forgets.records.is_empty());
  }

  /// B8: no record for an agent session another kept session of the same
  /// host and agent refers to, by its id or among its recorded pairs; a
  /// tombstone refers to nothing.
  #[test]
  fn no_record_for_an_agent_session_another_kept_session_refers_to() {
      let dir = tempfile::tempdir().unwrap();
      let (store, _db) = file_store(dir.path());
      started(&store, "s1", "claude", "a1", Some("/h/.claude"));
      started(&store, "s2", "claude", "a1", Some("/h/.claude"));
      let forgets = delete(&store, "s1");
      assert_eq!((forgets.shared, forgets.records.len()), (1, 0));

      // Among the recorded pairs, not the current id.
      started(&store, "s3", "claude", "a3", Some("/h/.claude"));
      store.mark_failed("s3", "x").unwrap();
      store
          .ingest("s3", 2, &started_with("s3", "a4", Some(home("/h/.claude"))))
          .unwrap();
      started(&store, "s4", "claude", "a3", Some("/h/.claude"));
      let forgets = delete(&store, "s4");
      assert_eq!((forgets.shared, forgets.records.len()), (1, 0));

      // By the current id alone: a session from before 9d records no pairs.
      started(&store, "s7", "claude", "a9", None);
      started(&store, "s8", "claude", "a9", Some("/h/.claude"));
      let forgets = delete(&store, "s8");
      assert_eq!((forgets.shared, forgets.records.len()), (1, 0));

      // Another agent or another host does not share it.
      store
          .create_session("s5", "h2", "claude", "/srv", "hat-1", None)
          .unwrap();
      store
          .ingest("s5", 1, &started_with("s5", "a1", Some(home("/h/.claude"))))
          .unwrap();
      store
          .create_session("s6", "h1", "codex", "/srv", "hat-1", None)
          .unwrap();
      store
          .ingest("s6", 1, &started_with("s6", "a1", Some(home("/h/.codex"))))
          .unwrap();
      // s1's tombstone no longer refers to a1: s2 is the last.
      let forgets = delete(&store, "s2");
      assert_eq!((forgets.shared, forgets.records.len()), (0, 1));
  }

  /// Decision 6, O10: a complete removal deletes its record; a result that
  /// no retry changes makes it final; anything else keeps it pending, its
  /// attempts counted only when sent; a dismissed record is gone.
  #[test]
  fn a_records_attempts_decide_whether_it_stays() {
      let dir = tempfile::tempdir().unwrap();
      let (store, _db) = file_store(dir.path());
      for (id, a) in [("s1", "a1"), ("s2", "a2"), ("s3", "a3")] {
          started(&store, id, "claude", a, Some("/h/.claude"));
          delete(&store, id);
      }
      let ids: Vec<String> = store.forgets_to_send("h1").unwrap().into_iter().map(|r| r.id).collect();
      let [r1, r2, r3] = ids.as_slice() else {
          panic!("{ids:?}");
      };
      store
          .forget_attempted(r1, &pending_result(RemovalPending::HostOffline), false, false)
          .unwrap();
      store
          .forget_attempted(r1, &pending_result(RemovalPending::NoReply), true, false)
          .unwrap();
      let removed = TranscriptRemoval {
          state: RemovalState::Removed,
          pending: None,
          remaining: Vec::new(),
          notes: Vec::new(),
      };
      store.forget_attempted(r2, &removed, true, true).unwrap();
      let left = hennery_sessions::store::final_result(ForgetReason::Symlink);
      store.forget_attempted(r3, &left, true, true).unwrap();

      let all = store.host_removals().unwrap();
      let states: Vec<(&str, HostRemovalState, u32)> = all.iter().map(|r| (r.id.as_str(), r.state, r.attempts)).collect();
      assert_eq!(
          states,
          [
              (r1.as_str(), HostRemovalState::Pending, 1),
              (r3.as_str(), HostRemovalState::Final, 1)
          ]
      );
      assert_eq!(all[0].last_result, Some(pending_result(RemovalPending::NoReply)));
      // A final record is not retried, nor changed by a late attempt.
      store
          .forget_attempted(r3, &pending_result(RemovalPending::NoReply), true, false)
          .unwrap();
      assert_eq!(store.host_removals().unwrap()[1].last_result, Some(left));
      assert_eq!(store.forgets_to_send("h1").unwrap().len(), 1);
      assert!(store.dismiss_forget(r3).unwrap());
      assert!(!store.dismiss_forget(r3).unwrap());
      assert_eq!(store.host_removals().unwrap().len(), 1);
  }

  /// O10: a revoked host's records are final, those it had and those a
  /// delete writes after.
  #[test]
  fn a_revoked_hosts_records_are_final() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
      let enrollment = hennery_kernel::hosts::Enrollment {
          public_key: "aa".repeat(32),
          name: "h".into(),
          host_version: "t".into(),
          platform: "t".into(),
      };
      hosts.register("h1", &enrollment, 0).unwrap();
      started(&store, "s1", "claude", "a1", Some("/h/.claude"));
      started(&store, "s2", "claude", "a2", Some("/h/.claude"));
      delete(&store, "s1");
      hosts.revoke("h1", 1).unwrap();
      store.revoke_host("h1").unwrap();
      store.close_now("s2").unwrap();
      let Deletion::Done { forgets, .. } = store.delete_session("s2", None).unwrap() else {
          panic!("not deleted");
      };
      assert_eq!(forgets.records[0].state, HostRemovalState::Final);
      let all = store.host_removals().unwrap();
      assert!(all.iter().all(|r| r.state == HostRemovalState::Final), "{all:?}");
      assert!(
          all.iter()
              .all(|r| r.last_result.as_ref().unwrap().remaining[0].reason == ForgetReason::HostRevoked)
      );
      assert!(store.forgets_to_send("h1").unwrap().is_empty());
  }

  fn stored_home(db: &Path, id: &str) -> Option<String> {
      Connection::open(db)
          .unwrap()
          .query_row("SELECT agent_home FROM host_forgets WHERE id = ?1", [id], |r| r.get(0))
          .unwrap()
  }

  /// The review's item 12: a record keeps its roots only while it may still
  /// be sent: a final one, by an attempt, a revoke or at its writing, has
  /// none.
  #[test]
  fn a_final_record_keeps_no_roots() {
      let dir = tempfile::tempdir().unwrap();
      let (store, db) = file_store(dir.path());
      for (id, a) in [("s1", "a1"), ("s2", "a2"), ("s3", "a3")] {
          started(&store, id, "claude", a, Some("/h/.claude"));
      }
      delete(&store, "s1");
      delete(&store, "s2");
      let records = store.forgets_to_send("h1").unwrap();
      let (r1, r2) = (&records[0].id, &records[1].id);
      assert!(stored_home(&db, r1).is_some());
      let left = hennery_sessions::store::final_result(ForgetReason::Symlink);
      store.forget_attempted(r1, &left, true, true).unwrap();
      assert_eq!(stored_home(&db, r1), None);
      store
          .forget_attempted(r2, &pending_result(RemovalPending::NoReply), true, false)
          .unwrap();
      assert!(stored_home(&db, r2).is_some());
      let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
      let enrollment = hennery_kernel::hosts::Enrollment {
          public_key: "aa".repeat(32),
          name: "h".into(),
          host_version: "t".into(),
          platform: "t".into(),
      };
      hosts.register("h1", &enrollment, 0).unwrap();
      hosts.revoke("h1", 1).unwrap();
      store.revoke_host("h1").unwrap();
      assert_eq!(stored_home(&db, r2), None);
      let written = delete(&store, "s3");
      assert_eq!(written.records[0].agent_home, None);
      assert_eq!(stored_home(&db, &written.records[0].id), None);
  }
  ```

  In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
              DROP TABLE turn_attachments;
              DROP INDEX events_by_kind;
  ```

  with:

  ```rust
              DROP TABLE turn_attachments;
              DROP TABLE host_forgets;
              ALTER TABLE sessions DROP COLUMN agent_home;
              DROP INDEX events_by_kind;
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
               DROP TABLE turn_attachments;
               DROP INDEX events_by_kind;
  ```

  with:

  ```rust
               DROP TABLE turn_attachments;
               DROP TABLE host_forgets;
               ALTER TABLE sessions DROP COLUMN agent_home;
               DROP INDEX events_by_kind;
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
                  indexed,
              },
  ```

  with:

  ```rust
                  indexed,
                  agent_home: None,
              },
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              &SessionBody::SessionStarted {
                  request_id: "r0".into(),
                  agent_session_id: "a1".into(),
                  indexed: snapshot,
              },
          )
          .unwrap();
      let before = store.catalog("s1").unwrap().unwrap();
  ```

  with:

  ```rust
              &SessionBody::SessionStarted {
                  request_id: "r0".into(),
                  agent_session_id: "a1".into(),
                  indexed: snapshot,
                  agent_home: None,
              },
          )
          .unwrap();
      let before = store.catalog("s1").unwrap().unwrap();
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
                  indexed: snapshot,
              },
  ```

  with:

  ```rust
                  indexed: snapshot,
                  agent_home: None,
              },
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
           DROP TABLE turn_attachments;
           DROP INDEX attachments_by_hash;
  ```

  with:

  ```rust
           DROP TABLE turn_attachments;
           DROP TABLE host_forgets;
           ALTER TABLE sessions DROP COLUMN agent_home;
           DROP INDEX attachments_by_hash;
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              unconfirmed: false,
          } => event,
  ```

  with:

  ```rust
              unconfirmed: false,
              ..
          } => event,
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      assert!(
          before.iter().all(|(_, n)| *n > 0),
  ```

  with:

  ```rust
      // `host_forgets` is written by the delete itself (plan 9d decision 2):
      // empty before it.
      assert!(
          before.iter().all(|(t, n)| *n > 0 || t == "host_forgets"),
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              "sessions" | "events" => 1,
              _ => 0,
  ```

  with:

  ```rust
              "sessions" | "events" => 1,
              // s2 still uses the agent session s1 had (`a0`): nothing is
              // left to remove on the host for it (plan 9d B8).
              "host_forgets" => 0,
              _ => 0,
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
                  indexed: catalogue("opus", "plan"),
              },
  ```

  with:

  ```rust
                  indexed: catalogue("opus", "plan"),
                  agent_home: Some(hennery_proto::frames::AgentHome {
                      root: "/home/me/.claude".into(),
                      sqlite_root: None,
                  }),
              },
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      let Deletion::Done { event, unconfirmed } = store
  ```

  with:

  ```rust
      let Deletion::Done { event, unconfirmed, .. } = store
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
          )
      );

      // The columns no closed session holds set are cleared too.
  ```

  with:

  ```rust
          )
      );
      // The agent's recorded home goes too (plan 9d B8), after it was copied
      // into the session's forget record.
      let home: Option<String> = conn
          .query_row("SELECT agent_home FROM sessions WHERE id = 's1'", [], |r| r.get(0))
          .unwrap();
      assert_eq!(home, None);

      // The columns no closed session holds set are cleared too.
  ```

  In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              Deletion::Done { event, unconfirmed } => {
  ```

  with:

  ```rust
              Deletion::Done { event, unconfirmed, .. } => {
  ```

  In `crates/hennery-testkit/tests/auth.rs`, replace:

  ```rust
      ("POST", "/api/hats/hat-9/purge"),
      ("POST", "/api/auth/step-up/password"),
  ```

  with:

  ```rust
      ("POST", "/api/hats/hat-9/purge"),
      ("GET", "/api/settings/host-removals"),
      ("DELETE", "/api/settings/host-removals/f-1"),
      ("POST", "/api/auth/step-up/password"),
  ```

  Create `crates/hennery-testkit/tests/forget.rs`:

  ```rust
  //! The agent's own transcript on its host (plan 9d): a real collector, a
  //! real host and the fake adapter standing in for the agents, under roots
  //! of the test's own (never the operator's `~/.claude`).

  use hennery_host::agent_home::Registry;
  use hennery_host::identity::HostKey;
  use hennery_host::{AgentCommand, HostConfig};
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_proto::frames::{AgentHome, ForgetKind, ForgetReason};
  use hennery_proto::rest::{
      DeleteResult, HostItem, HostRemovalItem, HostRemovalState, RemovalPending, RemovalState, StartSessionResponse,
  };
  use hennery_sessions::{AppState, store::Store};
  use hennery_testkit::{FakeScript, SCRIPT_ENV};
  use serde_json::json;
  use std::net::SocketAddr;
  use std::path::{Path, PathBuf};
  use std::time::Duration;

  /// An agent session id as Claude's SDK makes them.
  const AGENT_SESSION: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

  fn host_key() -> HostKey {
      HostKey::from_seed([1; 32])
  }

  struct Collector {
      addr: SocketAddr,
      state: AppState,
  }

  impl Collector {
      async fn start(db: &Path) -> Self {
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let hosts = Hosts::open(db).unwrap();
          let enrollment = Enrollment {
              public_key: host_key().public_key_hex(),
              name: "test".into(),
              host_version: "test".into(),
              platform: "test".into(),
          };
          hosts.register("host-1", &enrollment, 0).unwrap();
          let state = AppState::new(Store::open(db).unwrap(), hosts, Operator::open(db).unwrap());
          tokio::spawn(hennery_sessions::serve(listener, state.clone()));
          Self { addr, state }
      }

      fn url(&self, path: &str) -> String {
          format!("http://{}{path}", self.addr)
      }

      fn client(&self) -> reqwest::Client {
          hennery_testkit::operator_client(&self.state.operator)
      }
  }

  /// The test's own data roots, canonical (`/var` is a link on macOS).
  struct Roots {
      _dir: tempfile::TempDir,
      base: PathBuf,
  }

  impl Roots {
      fn new() -> Self {
          let dir = tempfile::tempdir().unwrap();
          let base = std::fs::canonicalize(dir.path()).unwrap();
          for sub in ["claude", "codex", "host", "work"] {
              std::fs::create_dir(base.join(sub)).unwrap();
          }
          Self { _dir: dir, base }
      }

      fn claude(&self) -> PathBuf {
          self.base.join("claude")
      }

      fn codex(&self) -> PathBuf {
          self.base.join("codex")
      }

      fn data_dir(&self) -> PathBuf {
          self.base.join("host")
      }

      fn work(&self) -> PathBuf {
          self.base.join("work")
      }
  }

  fn fake(script: &FakeScript, env: &[(&str, &Path)]) -> AgentCommand {
      let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
      fake.env
          .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
      for (name, value) in env {
          fake.env.push((name.to_string(), value.to_string_lossy().into_owned()));
      }
      fake
  }

  /// The script for a fake answering with `id`.
  fn answering(id: &str) -> FakeScript {
      FakeScript {
          session_id: Some(id.into()),
          ..FakeScript::default()
      }
  }

  /// A host whose `claude` and `codex` agents are the fake, each under its
  /// own root, answering `session/new` with `script`'s id.
  fn host_config(collector: SocketAddr, roots: &Roots, script: &FakeScript) -> HostConfig {
      let mut cfg = HostConfig::new(
          format!("ws://{collector}/api/hosts/ws"),
          "host-1",
          host_key(),
          roots.data_dir(),
      );
      cfg.reconnect_min = Duration::from_millis(100);
      cfg.reconnect_max = Duration::from_millis(300);
      cfg.agents
          .insert("claude".into(), fake(script, &[("CLAUDE_CONFIG_DIR", &roots.claude())]));
      cfg.agents
          .insert("codex".into(), fake(script, &[("CODEX_HOME", &roots.codex())]));
      cfg
  }

  fn start_host(cfg: HostConfig) -> tokio::task::JoinHandle<()> {
      tokio::spawn(async move {
          hennery_host::run(cfg).await.unwrap();
      })
  }

  async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
  where
      F: FnMut() -> Fut,
      Fut: std::future::Future<Output = Option<T>>,
  {
      let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
      loop {
          if let Some(v) = probe().await {
              return v;
          }
          assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
          tokio::time::sleep(Duration::from_millis(50)).await;
      }
  }

  async fn connected(collector: &Collector, yes: bool) {
      let c = collector.client();
      let url = collector.url("/api/hosts");
      wait_for("the host's connection", || async {
          let hosts: Vec<HostItem> = c.get(&url).send().await.ok()?.json().await.ok()?;
          (hosts.iter().any(|h| h.host_id == "host-1" && h.connected) == yes).then_some(())
      })
      .await;
  }

  async fn start_session(collector: &Collector, agent: &str, roots: &Roots) -> String {
      let resp = collector
          .client()
          .post(collector.url("/api/sessions"))
          .json(&json!({ "host_id": "host-1", "agent": agent, "cwd": roots.work() }))
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 202, "{}", resp.text().await.unwrap());
      resp.json::<StartSessionResponse>().await.unwrap().session_id
  }

  /// `DELETE /api/sessions/{id}`: its status and its body as text.
  async fn delete(collector: &Collector, session: &str) -> (u16, String) {
      let resp = collector
          .client()
          .delete(collector.url(&format!("/api/sessions/{session}")))
          .timeout(Duration::from_secs(40))
          .send()
          .await
          .unwrap();
      (resp.status().as_u16(), resp.text().await.unwrap())
  }

  async fn deleted(collector: &Collector, session: &str) -> (DeleteResult, String) {
      let (status, body) = delete(collector, session).await;
      assert_eq!(status, 200, "{body}");
      (serde_json::from_str(&body).unwrap(), body)
  }

  async fn removals(collector: &Collector) -> Vec<HostRemovalItem> {
      collector
          .client()
          .get(collector.url("/api/settings/host-removals"))
          .send()
          .await
          .unwrap()
          .json()
          .await
          .unwrap()
  }

  /// The recorded homes of a session, as the store keeps them.
  fn recorded(db: &Path, session: &str) -> Option<serde_json::Value> {
      let conn = rusqlite::Connection::open(db).unwrap();
      conn.query_row("SELECT agent_home FROM sessions WHERE id = ?1", [session], |r| {
          r.get::<_, Option<String>>(0)
      })
      .unwrap()
      .map(|raw| serde_json::from_str(&raw).unwrap())
  }

  fn home(root: &Path) -> AgentHome {
      AgentHome {
          root: root.to_str().unwrap().into(),
          sqlite_root: None,
      }
  }

  /// Both timeouts in one place: a live host answers before the collector
  /// gives up on it.
  #[test]
  fn the_hosts_deadline_is_inside_the_collectors_wait() {
      assert!(hennery_host::forget::FORGET_DEADLINE < hennery_sessions::forget::FORGET_WAIT);
  }

  /// Decision 1, B1: a started session's host registers where its agent keeps
  /// its data, then reports it in `session_started`, and the collector
  /// records it on the session.
  #[tokio::test]
  async fn a_started_session_registers_its_agent_home_and_the_collector_records_it() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
      connected(&collector, true).await;
      let session = start_session(&collector, "claude", &roots).await;
      let root = roots.claude();
      assert_eq!(
          recorded(&db, &session),
          Some(json!([{ "agent_session_id": AGENT_SESSION, "root": root.to_str().unwrap() }]))
      );
      let registry = Registry::open(&roots.data_dir().join(hennery_host::agent_home::FILE)).unwrap();
      assert!(registry.contains("claude", AGENT_SESSION, &home(&root)).unwrap());
      assert!(
          !registry
              .contains("claude", AGENT_SESSION, &home(&roots.codex()))
              .unwrap()
      );
  }

  /// Decision 7, 9d-i's Codex rule: a Codex session's home is recorded, and
  /// its host answers `unsupported_agent`, retryable: the record stays
  /// pending, listed, its attempt counted. No root reaches the answer (B2).
  #[tokio::test]
  async fn a_codex_sessions_removal_is_left_pending_for_a_later_host() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
      connected(&collector, true).await;
      let session = start_session(&collector, "codex", &roots).await;
      let (result, body) = deleted(&collector, &session).await;
      let removal = result.host_transcript;
      assert_eq!(removal.state, RemovalState::Partial, "{body}");
      assert_eq!(
          removal.remaining.iter().map(|r| (r.kind, r.reason)).collect::<Vec<_>>(),
          [(ForgetKind::Session, ForgetReason::UnsupportedAgent)]
      );
      assert!(removal.notes.is_empty());
      assert!(!body.contains(roots.base.to_str().unwrap()), "{body}");
      let listed = removals(&collector).await;
      let [item] = listed.as_slice() else {
          panic!("{listed:?}");
      };
      // The delete's own attempt; the close's `session_closed` may ask for
      // one more (the review's item 11). One or two, never a runaway loop.
      assert_eq!(
          (item.session_id.as_str(), item.agent.as_str(), item.state),
          (session.as_str(), "codex", HostRemovalState::Pending)
      );
      assert!((1..=2).contains(&item.attempts), "{item:?}");
  }

  /// Decision 5: a delete while the host is away answers `pending,
  /// host_offline`; the record goes when the host is back.
  #[tokio::test]
  async fn a_delete_while_the_host_is_away_is_pending_and_retried_at_its_return() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      let cfg = host_config(collector.addr, &roots, &answering(AGENT_SESSION));
      let host = start_host(cfg.clone());
      connected(&collector, true).await;
      let session = start_session(&collector, "codex", &roots).await;
      host.abort();
      connected(&collector, false).await;
      let (result, body) = deleted(&collector, &session).await;
      assert_eq!(
          (result.host_transcript.state, result.host_transcript.pending),
          (RemovalState::Pending, Some(RemovalPending::HostOffline)),
          "{body}"
      );
      let listed = removals(&collector).await;
      assert_eq!((listed[0].state, listed[0].attempts), (HostRemovalState::Pending, 0));
      start_host(cfg);
      wait_for("the retry at the host's return", || async {
          let listed = removals(&collector).await;
          (listed[0].attempts == 1).then_some(())
      })
      .await;
  }

  /// B1: a forget acts only on a home the host registered. A collector
  /// that names another root (here, its stored home rewritten) is answered
  /// `unknown_to_host`, final, and nothing there is touched.
  #[tokio::test]
  async fn a_root_the_host_never_registered_is_refused() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
      connected(&collector, true).await;
      let session = start_session(&collector, "claude", &roots).await;
      let elsewhere = roots.base.join("elsewhere");
      std::fs::create_dir_all(elsewhere.join("projects/p")).unwrap();
      let victim = elsewhere.join(format!("projects/p/{AGENT_SESSION}.jsonl"));
      std::fs::write(&victim, "keep").unwrap();
      let lie = json!([{ "agent_session_id": AGENT_SESSION, "root": elsewhere.to_str().unwrap() }]);
      rusqlite::Connection::open(&db)
          .unwrap()
          .execute(
              "UPDATE sessions SET agent_home = ?1 WHERE id = ?2",
              [lie.to_string(), session.clone()],
          )
          .unwrap();
      let (result, body) = deleted(&collector, &session).await;
      assert_eq!(
          result
              .host_transcript
              .remaining
              .iter()
              .map(|r| r.reason)
              .collect::<Vec<_>>(),
          [ForgetReason::UnknownToHost],
          "{body}"
      );
      assert_eq!(result.host_transcript.state, RemovalState::Partial);
      assert_eq!(removals(&collector).await[0].state, HostRemovalState::Final);
      assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
  }

  /// Decision 8, O10: an agent session id that is not one the agent writes
  /// is refused `invalid` before anything is looked at: final.
  #[tokio::test]
  async fn an_invalid_agent_session_id_is_refused_for_good() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      // The fake's default id, `fake-session-1`, is no UUID.
      start_host(host_config(collector.addr, &roots, &FakeScript::default()));
      connected(&collector, true).await;
      let session = start_session(&collector, "claude", &roots).await;
      let (result, body) = deleted(&collector, &session).await;
      assert_eq!(
          result
              .host_transcript
              .remaining
              .iter()
              .map(|r| r.reason)
              .collect::<Vec<_>>(),
          [ForgetReason::InvalidId],
          "{body}"
      );
      assert_eq!(removals(&collector).await[0].state, HostRemovalState::Final);
  }

  /// B8: an agent session another kept session still refers to is not
  /// forgotten: `shared`, and no record.
  #[tokio::test]
  async fn an_agent_session_another_session_still_uses_is_left_shared() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
      connected(&collector, true).await;
      let first = start_session(&collector, "claude", &roots).await;
      let _second = start_session(&collector, "claude", &roots).await;
      let (result, body) = deleted(&collector, &first).await;
      assert_eq!(result.host_transcript.state, RemovalState::Partial, "{body}");
      assert_eq!(
          result
              .host_transcript
              .remaining
              .iter()
              .map(|r| r.reason)
              .collect::<Vec<_>>(),
          [ForgetReason::Shared]
      );
      assert!(removals(&collector).await.is_empty());
  }

  /// `GET /api/settings/host-removals` lists what is left; a step-up
  /// `DELETE` dismisses one.
  #[tokio::test]
  async fn a_host_removal_is_listed_until_it_is_dismissed() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      start_host(host_config(collector.addr, &roots, &FakeScript::default()));
      connected(&collector, true).await;
      let session = start_session(&collector, "claude", &roots).await;
      deleted(&collector, &session).await;
      let listed = removals(&collector).await;
      assert_eq!(listed.len(), 1);
      // The Claude notes come with every listed Claude result.
      let notes = &listed[0].last_result.as_ref().unwrap().notes;
      assert!(notes.iter().any(|n| n.contains("context clear")), "{notes:?}");
      let url = collector.url(&format!("/api/settings/host-removals/{}", listed[0].id));
      let resp = collector.client().delete(&url).send().await.unwrap();
      assert_eq!(resp.status(), 204);
      assert!(removals(&collector).await.is_empty());
      assert_eq!(collector.client().delete(&url).send().await.unwrap().status(), 404);
  }
  ```

  Create `crates/hennery-testkit/tests/forget_host.rs`:

  ```rust
  //! `forget_session` on a real host, played frame by frame by a fake
  //! collector (plan 9d decisions 8, 13, B1, B7): the checks a forget passes
  //! before anything is removed.

  use futures::{SinkExt, StreamExt};
  use hennery_host::identity::HostKey;
  use hennery_host::{AgentCommand, HostConfig};
  use hennery_proto::frames::{
      AgentHome, Capability, CollectorFrame, ForgetKind, ForgetOutcome, ForgetReason, HostFrame, SessionBody,
  };
  use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
  use hennery_testkit::{FakeScript, SCRIPT_ENV};
  use std::collections::BTreeMap;
  use std::path::{Path, PathBuf};
  use std::time::Duration;
  use tokio::net::TcpListener;
  use tokio_tungstenite::tungstenite::Message;

  const AGENT_SESSION: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

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

      /// The next session fact, acked.
      async fn fact(&mut self) -> SessionBody {
          loop {
              if let HostFrame::Session { session_id, seq, body } = self.next().await {
                  self.send(&CollectorFrame::Ack {
                      session_id,
                      ack_seq: seq,
                  })
                  .await;
                  return body;
              }
          }
      }

      /// The answer to the forget `request_id`: a `session_forgotten`, or an
      /// `error` (as its code).
      async fn forget(&mut self, request_id: &str, agent_session_id: &str, root: &Path) -> Result<HostFrame, String> {
          self.send(&CollectorFrame::ForgetSession {
              request_id: request_id.into(),
              agent: "claude".into(),
              agent_session_id: agent_session_id.into(),
              agent_home: home(root),
          })
          .await;
          loop {
              match self.next().await {
                  frame @ HostFrame::SessionForgotten { .. } if frame.probe_request_id() == Some(request_id) => {
                      return Ok(frame);
                  }
                  HostFrame::Error {
                      request_id: r, code, ..
                  } if r == request_id => return Err(code),
                  HostFrame::Session { session_id, seq, .. } => {
                      self.send(&CollectorFrame::Ack {
                          session_id,
                          ack_seq: seq,
                      })
                      .await
                  }
                  _ => {}
              }
          }
      }
  }

  fn home(root: &Path) -> AgentHome {
      AgentHome {
          root: root.to_str().unwrap().into(),
          sqlite_root: None,
      }
  }

  /// The reasons of a `session_forgotten`'s remaining entries, with their
  /// retry flags.
  fn reasons(frame: &HostFrame) -> Vec<(ForgetKind, ForgetReason, bool)> {
      match frame {
          HostFrame::SessionForgotten { remaining, .. } => {
              remaining.iter().map(|r| (r.what.kind, r.reason, r.retry)).collect()
          }
          other => panic!("not a session_forgotten: {other:?}"),
      }
  }

  struct Setup {
      _dir: tempfile::TempDir,
      base: PathBuf,
      listener: TcpListener,
  }

  impl Setup {
      async fn new() -> Self {
          let dir = tempfile::tempdir().unwrap();
          let base = std::fs::canonicalize(dir.path()).unwrap();
          for sub in ["claude", "host", "work"] {
              std::fs::create_dir(base.join(sub)).unwrap();
          }
          let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
          Self {
              _dir: dir,
              base,
              listener,
          }
      }

      fn root(&self) -> PathBuf {
          self.base.join("claude")
      }

      fn data_dir(&self) -> PathBuf {
          self.base.join("host")
      }

      fn start_host(&self) {
          let script = FakeScript {
              session_id: Some(AGENT_SESSION.into()),
              ..FakeScript::default()
          };
          let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
          fake.env
              .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
          fake.env
              .push(("CLAUDE_CONFIG_DIR".into(), self.root().to_string_lossy().into_owned()));
          let mut cfg = HostConfig::new(
              format!("ws://{}/api/hosts/ws", self.listener.local_addr().unwrap()),
              "host-1",
              HostKey::from_seed([1; 32]),
              self.data_dir(),
          );
          cfg.agents.insert("claude".into(), fake);
          tokio::spawn(async move {
              let _ = hennery_host::run(cfg).await;
          });
      }

      fn start(&self) -> CollectorFrame {
          CollectorFrame::StartSession {
              request_id: "r1".into(),
              session_id: "s1".into(),
              committed_seq: 0,
              agent: "claude".into(),
              cwd: self.base.join("work").to_str().unwrap().into(),
              config: Default::default(),
          }
      }
  }

  /// Decision 13, B7, B1, decision 8: a forget is refused while a live actor
  /// has the agent's session, for a root the host never registered, and for
  /// an id the agent never writes; once the session is closed it runs.
  #[tokio::test]
  async fn a_forget_runs_only_for_a_registered_home_with_no_live_actor() {
      let setup = Setup::new().await;
      setup.start_host();
      let (mut collector, capabilities) = Collector::accept(&setup.listener).await;
      assert!(capabilities.contains(&Capability::ForgetSession));
      collector.send(&setup.start()).await;
      let SessionBody::SessionStarted { agent_home, .. } = collector.fact().await else {
          panic!("expected session_started");
      };
      assert_eq!(agent_home, Some(home(&setup.root())));

      let attached = collector.forget("f1", AGENT_SESSION, &setup.root()).await.unwrap();
      assert_eq!(
          reasons(&attached),
          [(ForgetKind::Session, ForgetReason::Attached, true)]
      );

      collector
          .send(&CollectorFrame::CloseSession {
              request_id: "c1".into(),
              session_id: "s1".into(),
          })
          .await;
      loop {
          if let SessionBody::SessionClosed = collector.fact().await {
              break;
          }
      }
      let unknown = collector
          .forget("f2", AGENT_SESSION, &setup.base.join("elsewhere"))
          .await
          .unwrap();
      assert_eq!(
          reasons(&unknown),
          [(ForgetKind::Session, ForgetReason::UnknownToHost, false)]
      );
      let other_id = "1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";
      let unknown = collector.forget("f3", other_id, &setup.root()).await.unwrap();
      assert_eq!(
          reasons(&unknown),
          [(ForgetKind::Session, ForgetReason::UnknownToHost, false)]
      );
      assert_eq!(
          collector.forget("f4", "../../etc", &setup.root()).await,
          Err("invalid".into())
      );
      let ran = collector.forget("f5", AGENT_SESSION, &setup.root()).await.unwrap();
      let HostFrame::SessionForgotten { outcome, .. } = &ran else {
          unreachable!()
      };
      assert_eq!(*outcome, ForgetOutcome::Partial);
      assert_eq!(
          reasons(&ran),
          [(ForgetKind::Session, ForgetReason::UnsupportedAgent, true)]
      );
  }

  /// Decision 1, B1: a home the host cannot register is never reported, so
  /// no forget can name it.
  #[tokio::test]
  async fn a_home_the_host_could_not_register_is_not_reported() {
      let setup = Setup::new().await;
      // A registry whose every write fails.
      rusqlite::Connection::open(setup.data_dir().join(hennery_host::agent_home::FILE))
          .unwrap()
          .execute_batch(
              "CREATE TABLE agent_homes (agent TEXT, agent_session_id TEXT, root TEXT, sqlite_root TEXT,
                   recorded_at INTEGER, CHECK (0));",
          )
          .unwrap();
      setup.start_host();
      let (mut collector, _) = Collector::accept(&setup.listener).await;
      collector.send(&setup.start()).await;
      let SessionBody::SessionStarted { agent_home, .. } = collector.fact().await else {
          panic!("expected session_started");
      };
      assert_eq!(agent_home, None);
  }

  /// Decision 1, B8: a load of an agent session that was registered under
  /// another root registers this one too, and says so in a `host_note`.
  #[tokio::test]
  async fn a_load_under_another_root_registers_both_and_says_so() {
      let setup = Setup::new().await;
      let registry =
          hennery_host::agent_home::Registry::open(&setup.data_dir().join(hennery_host::agent_home::FILE)).unwrap();
      registry
          .record("claude", AGENT_SESSION, &home(&setup.base.join("work")))
          .unwrap();
      setup.start_host();
      let (mut collector, _) = Collector::accept(&setup.listener).await;
      collector
          .send(&CollectorFrame::ResumeSession {
              request_id: "r1".into(),
              session_id: "s1".into(),
              committed_seq: 0,
              agent: "claude".into(),
              cwd: setup.base.join("work").to_str().unwrap().into(),
              agent_session_id: AGENT_SESSION.into(),
              config: Default::default(),
          })
          .await;
      let SessionBody::SessionStarted { agent_home, .. } = collector.fact().await else {
          panic!("expected session_started");
      };
      assert_eq!(agent_home, Some(home(&setup.root())));
      let SessionBody::HostNote { note, text } = collector.fact().await else {
          panic!("expected host_note");
      };
      assert_eq!(note, "agent_home_moved");
      assert!(!text.contains(setup.base.to_str().unwrap()), "{text}");
      assert!(
          registry
              .contains("claude", AGENT_SESSION, &home(&setup.root()))
              .unwrap()
      );
      assert!(
          registry
              .contains("claude", AGENT_SESSION, &home(&setup.base.join("work")))
              .unwrap()
      );
  }
  ```

  In `crates/hennery-testkit/tests/host_connection.rs`, replace:

  ```rust
              Capability::ResolvePath
  ```

  with:

  ```rust
              Capability::ResolvePath,
              Capability::ForgetSession,
  ```

  In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
  /// closes it there first, then deletes it: 204, 404 after, off the list, its
  ```

  with:

  ```rust
  /// closes it there first, then deletes it: 200, 404 after, off the list, its
  ```

  In `crates/hennery-testkit/tests/images.rs`, replace:

  ```rust
      assert_eq!(call.await.unwrap().status(), 204);
  ```

  with:

  ```rust
      assert_eq!(call.await.unwrap().status(), 200);
  ```

  In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          115,
  ```

  with:

  ```rust
          128,
  ```

  In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          "the host's own database, on the host's machine, not `hennery.db`",
      ),
  ```

  with:

  ```rust
          "the host's own database, on the host's machine, not `hennery.db`",
      ),
      (
          "hennery-host/src/agent_home.rs",
          "the host's own registry of agent homes (plan 9d B1), on the host's machine, not `hennery.db`",
      ),
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              indexed: catalogue("default"),
          },
  ```

  with:

  ```rust
              indexed: catalogue("default"),
              agent_home: None,
          },
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let session = parked_session(&collector, &mut host).await;
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      assert_eq!(get(&client(&collector), session_url(&collector, &session)).await.0, 404);
  ```

  with:

  ```rust
      let session = parked_session(&collector, &mut host).await;
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 200, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      assert_eq!(get(&client(&collector), session_url(&collector, &session)).await.0, 404);
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      assert_eq!(collector.lifecycle(&session), "closed");
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      assert_eq!(get(&client(&collector), session_url(&collector, &session)).await.0, 404);
  ```

  with:

  ```rust
      assert_eq!(collector.lifecycle(&session), "closed");
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(status, 200, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
      assert_eq!(get(&client(&collector), session_url(&collector, &session)).await.0, 404);
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      let (status, body) = call.await.unwrap();
      assert_eq!(status, 204, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
  ```

  with:

  ```rust
      let (status, body) = call.await.unwrap();
      assert_eq!(status, 200, "{body}");
      assert_eq!(collector.state.store.find_session(&session).unwrap(), None);
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      assert_eq!(status, 204, "{body}");
  ```

  with:

  ```rust
      assert_eq!(status, 200, "{body}");
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          204
  ```

  with:

  ```rust
          200
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          assert_eq!((status, body["code"].as_str()), (404, Some("not_found")), "{path}");
      }
  }
  ```

  with:

  ```rust
          assert_eq!((status, body["code"].as_str()), (404, Some("not_found")), "{path}");
      }
  }

  // Plan 9d: the agent's own transcript on its host, against a scripted host.

  const AGENT_SESSION: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

  fn agent_home() -> hennery_proto::frames::AgentHome {
      hennery_proto::frames::AgentHome {
          root: "/h/.claude".into(),
          sqlite_root: None,
      }
  }

  /// A `claude` session started through the API, its `session_started`
  /// reporting `agent_home()`, then parked by the host.
  async fn parked_claude_session(collector: &Collector, host: &mut ScriptedHost) -> String {
      let c = client(collector);
      let url = collector.url("/api/sessions");
      let call =
          tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "claude", "cwd": "/tmp" })).await });
      let CollectorFrame::StartSession {
          request_id, session_id, ..
      } = host.next().await
      else {
          panic!("expected start_session");
      };
      host.emit(
          &session_id,
          SessionBody::SessionStarted {
              request_id,
              agent_session_id: AGENT_SESSION.into(),
              indexed: Default::default(),
              agent_home: Some(agent_home()),
          },
      )
      .await;
      assert_eq!(call.await.unwrap().0, 202);
      host.emit(
          &session_id,
          SessionBody::SessionParked {
              reason: hennery_proto::frames::ParkReason::Idle,
          },
      )
      .await;
      wait_for("parked", || async {
          (collector.lifecycle(&session_id) == "parked").then_some(())
      })
      .await;
      session_id
  }

  fn forgetting() -> Capabilities {
      Capabilities(vec![
          Capability::Park,
          Capability::ResolvePath,
          Capability::ForgetSession,
      ])
  }

  /// The next `forget_session`: its request id, after checking it names the
  /// session's agent, id and recorded home.
  async fn expect_forget(host: &mut ScriptedHost) -> String {
      let CollectorFrame::ForgetSession {
          request_id,
          agent,
          agent_session_id,
          agent_home: home,
      } = host.next().await
      else {
          panic!("expected forget_session");
      };
      assert_eq!(
          (agent.as_str(), agent_session_id.as_str(), home),
          ("claude", AGENT_SESSION, agent_home())
      );
      request_id
  }

  fn forgotten(request_id: String, remaining: Vec<hennery_proto::frames::ForgetRemaining>) -> HostFrame {
      use hennery_proto::frames::ForgetOutcome;
      HostFrame::SessionForgotten {
          request_id,
          outcome: if remaining.is_empty() {
              ForgetOutcome::Complete
          } else {
              ForgetOutcome::Partial
          },
          removed: vec![],
          remaining,
      }
  }

  async fn removals(collector: &Collector) -> Vec<hennery_proto::rest::HostRemovalItem> {
      let (status, body) = get(&client(collector), collector.url("/api/settings/host-removals")).await;
      assert_eq!(status, 200);
      serde_json::from_value(body).unwrap()
  }

  /// Decisions 5–7: right after the delete, its host is asked to forget the
  /// agent's session under its recorded home; a complete answer is the
  /// delete's `removed`, and the record is gone.
  #[tokio::test]
  async fn a_delete_asks_its_host_to_forget_and_answers_removed() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let request_id = expect_forget(&mut host).await;
      host.send(&forgotten(request_id, vec![])).await;
      let (status, body) = call.await.unwrap();
      assert_eq!(
          (status, body["host_transcript"]["state"].as_str()),
          (200, Some("removed")),
          "{body}"
      );
      assert!(
          body["host_transcript"]["notes"][0]
              .as_str()
              .unwrap()
              .contains("context clear"),
          "{body}"
      );
      assert!(removals(&collector).await.is_empty());
  }

  /// Decision 7, O10: a host still attached answers `attached`: the delete
  /// is pending for it, and once that session's `session_closed` comes the
  /// forget goes again.
  #[tokio::test]
  async fn a_forget_answered_attached_goes_again_after_the_sessions_session_closed() {
      use hennery_proto::frames::{ForgetKind, ForgetReason, ForgetRemaining, ForgetWhat};
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let request_id = expect_forget(&mut host).await;
      let attached = ForgetRemaining {
          what: ForgetWhat {
              kind: ForgetKind::Session,
              count: 0,
          },
          reason: ForgetReason::Attached,
          retry: true,
      };
      host.send(&forgotten(request_id, vec![attached])).await;
      let (status, body) = call.await.unwrap();
      assert_eq!(status, 200, "{body}");
      assert_eq!(
          (
              body["host_transcript"]["state"].as_str(),
              body["host_transcript"]["pending"].as_str()
          ),
          (Some("pending"), Some("attached")),
          "{body}"
      );
      assert_eq!(removals(&collector).await[0].attempts, 1);
      host.emit(&session, SessionBody::SessionClosed).await;
      let request_id = expect_forget(&mut host).await;
      host.send(&forgotten(request_id, vec![])).await;
      wait_for("the record removed", || async {
          removals(&collector).await.is_empty().then_some(())
      })
      .await;
  }

  /// Decision 4: a host that does not announce `forget_session` is never
  /// sent one: pending, `host_needs_update`.
  #[tokio::test]
  async fn a_host_without_the_capability_is_left_pending_for_an_update() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(
          (
              status,
              body["host_transcript"]["state"].as_str(),
              body["host_transcript"]["pending"].as_str()
          ),
          (200, Some("pending"), Some("host_needs_update")),
          "{body}"
      );
      let listed = removals(&collector).await;
      assert_eq!(listed[0].attempts, 0);
  }

  /// B2: an answer past the size caps is not stored: the record stays
  /// pending, `no_reply`.
  #[tokio::test]
  async fn an_answer_past_the_caps_is_not_taken() {
      use hennery_proto::frames::{ForgetKind, ForgetOutcome, ForgetWhat};
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let request_id = expect_forget(&mut host).await;
      let many = vec![
          ForgetWhat {
              kind: ForgetKind::Transcript,
              count: 1
          };
          hennery_sessions::forget::MAX_ANSWER_ITEMS + 1
      ];
      host.send(&HostFrame::SessionForgotten {
          request_id,
          outcome: ForgetOutcome::Complete,
          removed: many,
          remaining: vec![],
      })
      .await;
      let (status, body) = call.await.unwrap();
      assert_eq!(
          (status, body["host_transcript"]["pending"].as_str()),
          (200, Some("no_reply")),
          "{body}"
      );
      assert_eq!(removals(&collector).await.len(), 1);
  }

  /// B7: one attempt per record at a time. A record whose attempt is held
  /// elsewhere is answered `in_progress` and nothing is sent or stored; the
  /// holder goes again once its own answer is in (the review's item 11).
  #[tokio::test]
  async fn a_record_with_an_attempt_in_flight_is_not_sent_again() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let first = expect_forget(&mut host).await;
      // While the route's attempt waits for its answer, a second one for the
      // same record (as a handshake's would) is not sent.
      let record = collector.state.store.forgets_to_send(HOST).unwrap().remove(0);
      let second = hennery_sessions::forget::attempt(&collector.state, &record, Duration::from_secs(5)).await;
      assert_eq!(second.pending, Some(hennery_proto::rest::RemovalPending::InProgress));
      // The first is answered `attached`: asked again meanwhile, it goes
      // again at once.
      use hennery_proto::frames::{ForgetKind, ForgetReason, ForgetRemaining, ForgetWhat};
      let attached = ForgetRemaining {
          what: ForgetWhat {
              kind: ForgetKind::Session,
              count: 0,
          },
          reason: ForgetReason::Attached,
          retry: true,
      };
      host.send(&forgotten(first, vec![attached])).await;
      let first = expect_forget(&mut host).await;
      host.send(&forgotten(first, vec![])).await;
      let (status, body) = call.await.unwrap();
      assert_eq!(
          (status, body["host_transcript"]["state"].as_str()),
          (200, Some("removed")),
          "{body}"
      );
  }

  /// B8, the review's item 13: a record whose agent session another kept
  /// session took up since the delete is not sent: final, `shared`.
  #[tokio::test]
  async fn a_record_whose_agent_session_is_in_use_again_is_not_sent() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = parked_claude_session(&collector, &mut host).await;
      // No capability: the delete leaves the record pending.
      let (status, body) = delete(&client(&collector), session_url(&collector, &session)).await;
      assert_eq!(
          (status, body["host_transcript"]["state"].as_str()),
          (200, Some("pending")),
          "{body}"
      );
      let _again = parked_claude_session(&collector, &mut host).await;
      let record = collector.state.store.forgets_to_send(HOST).unwrap().remove(0);
      let result = hennery_sessions::forget::attempt(&collector.state, &record, Duration::from_secs(5)).await;
      assert_eq!(
          result.remaining.iter().map(|r| r.reason).collect::<Vec<_>>(),
          [hennery_proto::frames::ForgetReason::Shared]
      );
      let listed = removals(&collector).await;
      assert_eq!(
          (listed[0].state, listed[0].attempts),
          (hennery_proto::rest::HostRemovalState::Final, 0)
      );
  }
  ```

  In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
          ("DELETE", "/api/sessions/s-9".to_string(), None, 404),
          ("DELETE", format!("/api/auth/sessions/{}", c.id_of(&other)), None, 204),
  ```

  with:

  ```rust
          ("DELETE", "/api/sessions/s-9".to_string(), None, 404),
          // Dismissing a host removal (plan 9d O10); listing them is a read.
          ("DELETE", "/api/settings/host-removals/f-9".to_string(), None, 404),
          ("DELETE", format!("/api/auth/sessions/{}", c.id_of(&other)), None, 204),
  ```

  In `crates/hennery-testkit/tests/step_up.rs`, replace:

  ```rust
      assert_eq!(code_of(resp).await, (404, "not_found".into()));
      assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
  ```

  with:

  ```rust
      assert_eq!(code_of(resp).await, (404, "not_found".into()));
      let resp = send(&stale, "GET", "/api/settings/host-removals", None).await.unwrap();
      assert_eq!(resp.status(), 200);
      assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-sessions -p hennery-testkit --locked --test forget_store --test forget`
Expected: FAIL to compile: `cannot find type ForgetKind in module hennery_proto::frames`.

- [ ] **Step 3: The recording, the record and the protocol**

  Create `crates/hennery-host/src/agent_home.rs`:

  ```rust
  //! Where an agent keeps its own data for a session (plan 9d decision 1),
  //! and the host's registry of it (B1).
  //!
  //! On every start and resume the host resolves the agent's data roots from
  //! the environment the adapter runs with, and registers `(agent, agent
  //! session id, roots)` durably before it sends `session_started`. A forget
  //! acts only on an exact match of that registry, so a collector naming any
  //! other root is refused (`unknown_to_host`). Entries are kept after a
  //! forget: a retry after a lost answer checks again and answers the same.

  use anyhow::{Context, Result};
  use hennery_proto::frames::AgentHome;
  use rusqlite::{Connection, OptionalExtension, params};
  use std::path::{Path, PathBuf};
  use std::sync::Mutex;

  /// The registry's file in the host's data directory.
  pub const FILE: &str = "agent-homes.db";

  /// The agents whose data the host knows how to find.
  pub const CLAUDE: &str = "claude";
  pub const CODEX: &str = "codex";

  /// `name`'s value for the adapter: the agent's own configuration
  /// (`agent.env`, the last setting wins), then the host's environment,
  /// which the adapter inherits. Empty counts as unset, as both agents read
  /// it.
  fn lookup(env: &[(String, String)], name: &str) -> Option<String> {
      env.iter()
          .rev()
          .find(|(key, _)| key == name)
          .map(|(_, value)| value.clone())
          .or_else(|| std::env::var(name).ok())
          .filter(|value| !value.is_empty())
  }

  /// `path`, canonical and UTF-8, if it exists.
  fn canonical(path: &Path) -> Option<String> {
      match std::fs::canonicalize(path) {
          Ok(path) => path.into_os_string().into_string().ok(),
          Err(err) => {
              tracing::info!(path = %path.display(), error = %err, "an agent data root does not resolve; not recorded");
              None
          }
      }
  }

  /// The data roots `agent` uses with `env` (plan 9d decision 1): Claude's
  /// `CLAUDE_CONFIG_DIR`, else `$HOME/.claude`; Codex's `CODEX_HOME`, else
  /// `$HOME/.codex`, and `CODEX_SQLITE_HOME` if set. Canonical, as the
  /// filesystem gives the bytes (O11: no NFC). `None` for another agent, or
  /// a root that does not resolve (not made yet): such a session records no
  /// home, and its forget is final (decision 11).
  pub fn resolve(agent: &str, env: &[(String, String)]) -> Option<AgentHome> {
      let (var, default) = match agent {
          CLAUDE => ("CLAUDE_CONFIG_DIR", ".claude"),
          CODEX => ("CODEX_HOME", ".codex"),
          _ => return None,
      };
      let root = match lookup(env, var) {
          Some(root) => PathBuf::from(root),
          None => PathBuf::from(lookup(env, "HOME")?).join(default),
      };
      let sqlite_root = match (agent, lookup(env, "CODEX_SQLITE_HOME")) {
          (CODEX, Some(sqlite)) => Some(canonical(Path::new(&sqlite))?),
          _ => None,
      };
      Some(AgentHome {
          root: canonical(&root)?,
          sqlite_root,
      })
  }

  /// The host's registry of agent homes (B1), in its data directory.
  pub struct Registry {
      conn: Mutex<Connection>,
  }

  impl std::fmt::Debug for Registry {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.write_str("Registry")
      }
  }

  impl Registry {
      pub fn open(path: &Path) -> Result<Self> {
          Self::init(Connection::open(path).with_context(|| format!("open {}", path.display()))?)
      }

      pub fn open_in_memory() -> Result<Self> {
          Self::init(Connection::open_in_memory()?)
      }

      fn init(conn: Connection) -> Result<Self> {
          conn.pragma_update(None, "journal_mode", "WAL")?;
          // Each entry is on disk before `session_started` goes out (B1):
          // every commit is synced, whatever the build's default.
          conn.pragma_update(None, "synchronous", "FULL")?;
          conn.execute_batch(
              "CREATE TABLE IF NOT EXISTS agent_homes (
                   agent TEXT NOT NULL,
                   agent_session_id TEXT NOT NULL,
                   root TEXT NOT NULL,
                   sqlite_root TEXT NOT NULL DEFAULT '',
                   recorded_at INTEGER NOT NULL,
                   PRIMARY KEY (agent, agent_session_id, root, sqlite_root));",
          )?;
          Ok(Self { conn: Mutex::new(conn) })
      }

      /// Register `home` for `agent`'s session `agent_session_id`, durably.
      /// `true` if that session was registered with other roots before (a
      /// moved home: both stay registered, B8).
      pub fn record(&self, agent: &str, agent_session_id: &str, home: &AgentHome) -> Result<bool> {
          let conn = self.conn.lock().expect("registry lock");
          let sqlite = home.sqlite_root.as_deref().unwrap_or("");
          let moved: Option<i64> = conn
              .query_row(
                  "SELECT 1 FROM agent_homes WHERE agent = ?1 AND agent_session_id = ?2
                       AND NOT (root = ?3 AND sqlite_root = ?4) LIMIT 1",
                  params![agent, agent_session_id, home.root, sqlite],
                  |r| r.get(0),
              )
              .optional()?;
          let now = std::time::SystemTime::now()
              .duration_since(std::time::UNIX_EPOCH)
              .map(|d| d.as_secs() as i64)
              .unwrap_or(0);
          conn.execute(
              "INSERT INTO agent_homes(agent, agent_session_id, root, sqlite_root, recorded_at)
               VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT DO NOTHING",
              params![agent, agent_session_id, home.root, sqlite, now],
          )?;
          Ok(moved.is_some())
      }

      /// Whether exactly this `(agent, agent_session_id, home)` is registered
      /// (B1): byte for byte.
      pub fn contains(&self, agent: &str, agent_session_id: &str, home: &AgentHome) -> Result<bool> {
          let conn = self.conn.lock().expect("registry lock");
          Ok(conn
              .query_row(
                  "SELECT 1 FROM agent_homes WHERE agent = ?1 AND agent_session_id = ?2 AND root = ?3
                       AND sqlite_root = ?4",
                  params![
                      agent,
                      agent_session_id,
                      home.root,
                      home.sqlite_root.as_deref().unwrap_or("")
                  ],
                  |r| r.get::<_, i64>(0),
              )
              .optional()?
              .is_some())
      }
  }
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
  use crate::identity::HostKey;
  use crate::outbox::Outbox;
  use crate::projects::Probes;
  use crate::session::{self, AgentCommand, Answer, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
  use crate::uplink::Uplink;
  use anyhow::{Context, Result, bail};
  use futures::{SinkExt, StreamExt};
  use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionConfig};
  ```

  with:

  ```rust
  use crate::agent_home::Registry;
  use crate::forget::{Forget, ForgetContext};
  use crate::identity::HostKey;
  use crate::outbox::Outbox;
  use crate::projects::Probes;
  use crate::session::{
      self, AgentCommand, Answer, Attach, HomeRecorder, Launch, SessionCmd, SessionHandle, SessionOptions,
  };
  use crate::uplink::Uplink;
  use anyhow::{Context, Result, bail};
  use futures::{SinkExt, StreamExt};
  use hennery_proto::frames::{
      AttachedSession, Capabilities, Capability, CollectorFrame, ForgetReason, HostFrame, SessionConfig,
  };
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      closing: bool,
  }
  ```

  with:

  ```rust
      closing: bool,
      /// Agent session ids a forget is removing right now, and how many
      /// forgets each (plan 9d B7): no actor attaches one meanwhile.
      forgetting: HashMap<String, usize>,
  }
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      let (uplink, mut replies) = Uplink::new(outbox);
      let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
  ```

  with:

  ```rust
      let (uplink, mut replies) = Uplink::new(outbox);
      // Where each session's agent keeps its data (plan 9d B1).
      let homes = Arc::new(crate::agent_home::Registry::open(
          &cfg.data_dir.join(crate::agent_home::FILE),
      )?);
      let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              if let Err(err) = connect_once(&cfg, &uplink, &sessions, &probes, &mut replies, &mut backoff).await {
  ```

  with:

  ```rust
              if let Err(err) = connect_once(&cfg, &uplink, &sessions, &probes, &homes, &mut replies, &mut backoff).await
              {
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
                  Capability::ResolvePath,
              ]),
  ```

  with:

  ```rust
                  Capability::ResolvePath,
                  Capability::ForgetSession,
              ]),
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      probes: &Probes,
      replies: &mut mpsc::UnboundedReceiver<HostFrame>,
  ```

  with:

  ```rust
      probes: &Probes,
      homes: &Arc<Registry>,
      replies: &mut mpsc::UnboundedReceiver<HostFrame>,
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
                              handle(cfg, uplink, sessions, probes, frame)?
  ```

  with:

  ```rust
                              handle(cfg, uplink, sessions, probes, homes, frame)?
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
  fn attach(cfg: &HostConfig, uplink: &Uplink, sessions: &Sessions, req: AttachRequest) -> Result<()> {
  ```

  with:

  ```rust
  fn attach(
      cfg: &HostConfig,
      uplink: &Uplink,
      sessions: &Sessions,
      homes: &Arc<Registry>,
      req: AttachRequest,
  ) -> Result<()> {
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      let options = cfg.session_options();
  ```

  with:

  ```rust
      let mut options = cfg.session_options();
      // Registered before each `session_started` (plan 9d decision 1).
      options.home = Some(HomeRecorder {
          agent: req.agent.clone(),
          registry: homes.clone(),
      });
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          tracing::info!(session_id = %req.session_id, "host shutting down; not attaching");
          return;
  ```

  with:

  ```rust
          tracing::info!(session_id = %req.session_id, "host shutting down; not attaching");
          return;
      }
      // A forget is removing this agent session's data (plan 9d B7): it is
      // not loaded meanwhile. Checked on every re-decide too.
      if let Attach::Load { agent_session_id } = &req.attach
          && map.forgetting.contains_key(agent_session_id)
      {
          uplink.reply(HostFrame::Error {
              request_id: req.request_id,
              code: "forgetting".into(),
              message: "the agent's data for this session is being removed".into(),
          });
          return;
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      probes: &Probes,
      frame: CollectorFrame,
  ```

  with:

  ```rust
      probes: &Probes,
      homes: &Arc<Registry>,
      frame: CollectorFrame,
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              cwd,
              config,
          } => attach(
              cfg,
              uplink,
              sessions,
              AttachRequest {
                  request_id,
                  session_id,
                  committed_seq,
                  agent,
                  cwd,
  ```

  with:

  ```rust
              cwd,
              config,
          } => attach(
              cfg,
              uplink,
              sessions,
              homes,
              AttachRequest {
                  request_id,
                  session_id,
                  committed_seq,
                  agent,
                  cwd,
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              sessions,
              AttachRequest {
  ```

  with:

  ```rust
              sessions,
              homes,
              AttachRequest {
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
      }
      Ok(())
  ```

  with:

  ```rust
          CollectorFrame::ForgetSession {
              request_id,
              agent,
              agent_session_id,
              agent_home,
          } => forget(
              cfg,
              uplink,
              sessions,
              homes,
              request_id,
              Forget {
                  agent,
                  agent_session_id,
                  agent_home,
              },
          ),
          CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
      }
      Ok(())
  }

  /// `forget_session` (plan 9d decision 8): the id first, then the registry
  /// (B1), then, under the sessions lock, that no live actor has the agent's
  /// session (decision 13, B7: an actor still ending counts, it may still
  /// write). Then the forget runs in a task of its own, its marker refusing
  /// any attach of that agent session until it is done, and answers on this
  /// connection's reply channel.
  fn forget(
      cfg: &HostConfig,
      uplink: &Uplink,
      sessions: &Sessions,
      homes: &Arc<Registry>,
      request_id: String,
      f: Forget,
  ) {
      if !crate::forget::valid_id(&f.agent_session_id) {
          uplink.reply(HostFrame::Error {
              request_id,
              code: "invalid".into(),
              message: "not an agent session id".into(),
          });
          return;
      }
      match homes.contains(&f.agent, &f.agent_session_id, &f.agent_home) {
          Ok(true) => {}
          Ok(false) => {
              tracing::warn!(agent = %f.agent, "a forget for an agent home this host never registered; refused");
              return uplink.reply(crate::forget::refused(request_id, ForgetReason::UnknownToHost, false));
          }
          Err(err) => {
              tracing::error!(error = %err, "reading the agent-home registry failed");
              return uplink.reply(crate::forget::refused(request_id, ForgetReason::IoError, true));
          }
      }
      {
          let mut map = sessions.lock().expect("sessions lock");
          let attached = map
              .handles
              .values()
              .any(|h| !h.is_ended() && h.agent_session_id().as_deref() == Some(f.agent_session_id.as_str()));
          if attached {
              return uplink.reply(crate::forget::refused(request_id, ForgetReason::Attached, true));
          }
          // One forget per agent session at a time (the review's item 3).
          if map.forgetting.contains_key(&f.agent_session_id) {
              return uplink.reply(crate::forget::refused(request_id, ForgetReason::InProgress, true));
          }
          *map.forgetting.entry(f.agent_session_id.clone()).or_default() += 1;
      }
      let ctx = ForgetContext {
          agents: cfg.agents.clone(),
          data_dir: cfg.data_dir.clone(),
          home: cfg.home.clone(),
      };
      let (uplink, sessions) = (uplink.clone(), sessions.clone());
      tokio::spawn(async move {
          let done = crate::forget::forget(&ctx, &f).await;
          {
              let mut map = sessions.lock().expect("sessions lock");
              if let Some(n) = map.forgetting.get_mut(&f.agent_session_id) {
                  *n -= 1;
                  if *n == 0 {
                      map.forgetting.remove(&f.agent_session_id);
                  }
              }
          }
          uplink.reply(done.into_frame(request_id));
      });
  ```

  Create `crates/hennery-host/src/forget.rs`:

  ```rust
  //! `forget_session` on the host (plan 9d decisions 8, 11–13): remove a
  //! deleted session's transcript from the agent's own data, under the root
  //! the host itself registered for it (B1), and say what was removed and
  //! what is left, as kinds and counts (B2).
  //!
  //! The connection (`connection::forget`) checks the id, the registry and
  //! that no live actor has the agent's session (B7), then runs `forget` in a
  //! task of its own, holding a marker that refuses any attach of that agent
  //! session meanwhile.

  use crate::adapter::AgentCommand;
  use hennery_proto::frames::{ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
  use std::collections::HashMap;
  use std::path::PathBuf;
  use std::time::Duration;

  /// One deadline for a whole forget on the host (B6): below the collector's
  /// wait (`hennery_sessions::forget::FORGET_WAIT`, 30 s), so its answer
  /// comes first.
  pub const FORGET_DEADLINE: Duration = Duration::from_secs(20);

  /// What a forget needs of the host.
  #[derive(Debug, Clone)]
  pub struct ForgetContext {
      /// The host's agents, by name: the adapter a forget runs (decision 8).
      pub agents: HashMap<String, AgentCommand>,
      /// The host's data directory: never a root (B3).
      pub data_dir: PathBuf,
      /// The host user's home directory, if known: never a root, nor an
      /// ancestor of it (B3). `HOME` counts too.
      pub home: Option<PathBuf>,
  }

  /// One forget, as the collector asked for it, checked against the
  /// registry already.
  #[derive(Debug, Clone)]
  pub struct Forget {
      pub agent: String,
      pub agent_session_id: String,
      pub agent_home: hennery_proto::frames::AgentHome,
  }

  /// Whether `id` is an id the agent itself writes (decision 8): a UUID in
  /// lowercase hex, as Claude's SDK names its files. Nothing else is ever
  /// built into a path.
  pub fn valid_id(id: &str) -> bool {
      let bytes = id.as_bytes();
      bytes.len() == 36
          && bytes.iter().enumerate().all(|(i, b)| match i {
              8 | 13 | 18 | 23 => *b == b'-',
              _ => b.is_ascii_digit() || (b'a'..=b'f').contains(b),
          })
  }

  pub fn left(kind: ForgetKind, count: u32, reason: ForgetReason, retry: bool) -> ForgetRemaining {
      ForgetRemaining {
          what: ForgetWhat { kind, count },
          reason,
          retry,
      }
  }

  /// The answer for a forget that could not start: the whole session left,
  /// for `reason`.
  pub fn refused(request_id: String, reason: ForgetReason, retry: bool) -> HostFrame {
      HostFrame::SessionForgotten {
          request_id,
          outcome: ForgetOutcome::Partial,
          removed: Vec::new(),
          remaining: vec![left(ForgetKind::Session, 0, reason, retry)],
      }
  }

  /// What one forget did.
  #[derive(Debug, Clone, PartialEq)]
  pub struct Forgotten {
      pub removed: Vec<ForgetWhat>,
      pub remaining: Vec<ForgetRemaining>,
  }

  impl Forgotten {
      pub fn into_frame(self, request_id: String) -> HostFrame {
          HostFrame::SessionForgotten {
              request_id,
              outcome: if self.remaining.is_empty() {
                  ForgetOutcome::Complete
              } else {
                  ForgetOutcome::Partial
              },
              removed: self.removed,
              remaining: self.remaining,
          }
      }
  }

  /// Run one forget (plan 9d decision 8). Only Claude's data is removed so
  /// far; any other agent is answered `unsupported_agent`, retryable, for
  /// plan 9d-ii to take up.
  pub async fn forget(_ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      let _ = &forget.agent_home;
      Forgotten {
          removed: Vec::new(),
          remaining: vec![left(ForgetKind::Session, 0, ForgetReason::UnsupportedAgent, true)],
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn only_a_lowercase_uuid_is_an_agent_session_id() {
          assert!(valid_id("0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3"));
          for bad in [
              "0B9C1D2E-3F40-4A5B-8C6D-7E8F90A1B2C3",
              "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c",
              "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3x",
              "0b9c1d2e_3f40-4a5b-8c6d-7e8f90a1b2c3",
              "../../../../../../etc/passwd/aaaaaaa",
              "fake-session-1",
              "",
              "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2g3",
          ] {
              assert!(!valid_id(bad), "{bad:?}");
          }
      }
  }
  ```

  In `crates/hennery-host/src/lib.rs`, replace:

  ```rust
  pub mod connection;
  ```

  with:

  ```rust
  pub mod agent_home;
  pub mod connection;
  pub mod forget;
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
      /// Test seams (see `test_hooks`).
      #[cfg(feature = "test-hooks")]
      pub test_hooks: Option<test_hooks::TestHooks>,
  ```

  with:

  ```rust
      /// Where the agent's data is registered before each `session_started`
      /// (plan 9d decision 1, B1); `None`: nothing is recorded.
      pub home: Option<HomeRecorder>,
      /// Test seams (see `test_hooks`).
      #[cfg(feature = "test-hooks")]
      pub test_hooks: Option<test_hooks::TestHooks>,
  }

  /// What a session actor needs to record its agent's home (plan 9d
  /// decision 1): the agent's name, as the collector sends it, and the
  /// host's registry.
  #[derive(Debug, Clone)]
  pub struct HomeRecorder {
      pub agent: String,
      pub registry: Arc<crate::agent_home::Registry>,
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
              git: None,
              #[cfg(feature = "test-hooks")]
  ```

  with:

  ```rust
              git: None,
              home: None,
              #[cfg(feature = "test-hooks")]
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
  }

  impl SessionHandle {
  ```

  with:

  ```rust
      /// The agent's own session id, once known: at launch for a load, once
      /// `session/new` answered for a new session (plan 9d B7).
      agent_session_id: Arc<Mutex<Option<String>>>,
  }

  impl SessionHandle {
      /// The agent's own session id, if known yet (plan 9d B7): a forget for
      /// it is refused while this actor has not ended.
      pub fn agent_session_id(&self) -> Option<String> {
          self.agent_session_id.lock().expect("agent session id lock").clone()
      }

  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
      let actor = Actor {
  ```

  with:

  ```rust
      let agent_session_id = Arc::new(Mutex::new(match &launch.attach {
          Attach::Load { agent_session_id } => Some(agent_session_id.clone()),
          Attach::New => None,
      }));
      let actor = Actor {
          agent_session_id: agent_session_id.clone(),
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
          ending,
      }
  ```

  with:

  ```rust
          ending,
          agent_session_id,
      }
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
      ending: Arc<AtomicBool>,
  }
  ```

  with:

  ```rust
      ending: Arc<AtomicBool>,
      /// Shared with the handle (`SessionHandle::agent_session_id`).
      agent_session_id: Arc<Mutex<Option<String>>>,
  }
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
              tracing::error!(session_id = %self.session_id, error = %err, "failed to persist a session frame");
          }
  ```

  with:

  ```rust
              tracing::error!(session_id = %self.session_id, error = %err, "failed to persist a session frame");
          }
      }

      /// Emit `session_started` for `request_id`, after registering where the
      /// agent keeps this session's data (plan 9d decision 1, B1): durably,
      /// before the frame, so a forget can always match it. A home that
      /// cannot be resolved or registered is left out, and the session
      /// records none. A load whose agent session was registered with other
      /// roots before gets a `host_note` after it: both stay registered (B8).
      fn announce(&self, request_id: String, agent_session: &str, env: &[(String, String)], loaded: bool) {
          *self.agent_session_id.lock().expect("agent session id lock") = Some(agent_session.to_string());
          let mut moved = false;
          let agent_home = self.options.home.as_ref().and_then(|recorder| {
              let home = crate::agent_home::resolve(&recorder.agent, env)?;
              match recorder.registry.record(&recorder.agent, agent_session, &home) {
                  Ok(was_elsewhere) => {
                      moved = was_elsewhere;
                      Some(home)
                  }
                  Err(err) => {
                      tracing::error!(session_id = %self.session_id, error = %err, "registering the agent's home failed; not reported");
                      None
                  }
              }
          });
          self.emit(SessionBody::SessionStarted {
              request_id,
              agent_session_id: agent_session.to_string(),
              indexed: self.catalogue_extracts(),
              agent_home,
          });
          if moved && loaded {
              self.emit(SessionBody::HostNote {
                  note: "agent_home_moved".into(),
                  text: "the agent's data directory is not the one this session used before; both are recorded".into(),
              });
          }
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
          let probe_cwd = cwd.clone();
          let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
  ```

  with:

  ```rust
          let probe_cwd = cwd.clone();
          // Where the agent's data is resolved from (plan 9d decision 1).
          let agent_env = agent.env.clone();
          let loaded = matches!(attach, Attach::Load { .. });
          let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
          self.emit(SessionBody::SessionStarted {
              request_id,
              agent_session_id: agent_session.to_string(),
              indexed: self.catalogue_extracts(),
          });
  ```

  with:

  ```rust
          self.announce(request_id, &agent_session.to_string(), &agent_env, loaded);
  ```

  In `crates/hennery-host/src/session.rs`, replace:

  ```rust
                      Some(SessionCmd::Restart { request_id }) => self.emit(SessionBody::SessionStarted {
                          request_id,
                          agent_session_id: agent_session.to_string(),
                          indexed: self.catalogue_extracts(),
                      }),
  ```

  with:

  ```rust
                      Some(SessionCmd::Restart { request_id }) => {
                          self.announce(request_id, &agent_session.to_string(), &agent_env, loaded)
                      }
  ```

  In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::McpMountsRequest,
          rest::McpCredentialRequest,
      );
      // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
  ```

  with:

  ```rust
          rest::McpMountsRequest,
          rest::McpCredentialRequest,
          rest::DeleteResult,
          rest::HostRemovalItem,
      );
      // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
  ```

  In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          frames::DirEntry,
          frames::HostFrame,
  ```

  with:

  ```rust
          frames::DirEntry,
          frames::AgentHome,
          frames::ForgetKind,
          frames::ForgetReason,
          frames::ForgetWhat,
          frames::ForgetRemaining,
          frames::ForgetOutcome,
          frames::HostFrame,
  ```

  In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::McpCredentialRequest,
      );
  ```

  with:

  ```rust
          rest::McpCredentialRequest,
          rest::RemovalState,
          rest::RemovalPending,
          rest::RemovalItem,
          rest::TranscriptRemoval,
          rest::DeleteResult,
          rest::HostRemovalState,
          rest::HostRemovalItem,
      );
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      ResolvePath,
  }
  ```

  with:

  ```rust
      ResolvePath,
      /// Removing a deleted session's transcript from the agent's own data
      /// (`forget_session`, plan 9d decision 4).
      ForgetSession,
  }

  /// The longest agent data root a host may report (plan 9d, O12's shape
  /// check): Linux's `PATH_MAX`.
  pub const AGENT_HOME_MAX_BYTES: usize = 4096;

  /// Where an agent keeps its own data for a session, as its host resolved
  /// it from the adapter's environment (plan 9d decision 1): `root` is
  /// Claude's `CLAUDE_CONFIG_DIR` or `~/.claude`, Codex's `CODEX_HOME` or
  /// `~/.codex`; `sqlite_root` is Codex's `CODEX_SQLITE_HOME` if set.
  /// Canonical, its bytes as the filesystem gave them (O11). A host's report
  /// is not verified by the collector beyond its shape (`is_well_formed`).
  #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
  pub struct AgentHome {
      pub root: String,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub sqlite_root: Option<String>,
  }

  impl AgentHome {
      /// Absolute, bounded and with no NUL, each path (plan 9d, the shape
      /// check the collector makes before it stores a home).
      pub fn is_well_formed(&self) -> bool {
          let ok = |p: &str| p.starts_with('/') && p.len() <= AGENT_HOME_MAX_BYTES && !p.contains('\0');
          ok(&self.root) && self.sqlite_root.as_deref().is_none_or(ok)
      }
  }

  /// What a forget names on the host (plan 9d decision 4, B2): a kind of
  /// entry, never a path. The masked path each stands for is `masked`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum ForgetKind {
      /// The whole forget, when it could not start (no home, an unknown id,
      /// an agent this host cannot forget for yet).
      Session,
      /// The transcript and its family in each project directory (B9).
      Transcript,
      FileHistory,
      SessionEnv,
      Tasks,
      Debug,
      /// Codex's own database copies of the conversation (plan 9d-ii).
      CodexDatabaseCopies,
  }

  impl ForgetKind {
      /// The entries this kind stands for, relative to the agent's root, with
      /// the project directory masked (B2).
      pub fn masked(self) -> &'static str {
          match self {
              Self::Session => "<session>",
              Self::Transcript => "projects/*/<id>.jsonl (and its family)",
              Self::FileHistory => "file-history/<id>/",
              Self::SessionEnv => "session-env/<id>/",
              Self::Tasks => "tasks/<id>/",
              Self::Debug => "debug/<id>.txt",
              Self::CodexDatabaseCopies => "<codex database>",
          }
      }
  }

  /// Why something named was not removed: a fixed code the host chooses
  /// (plan 9d B2), never free text.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum ForgetReason {
      /// A live actor on the host has that agent session id (decision 13, B7).
      Attached,
      /// Another forget of the same agent session is running on the host
      /// (B7; the review's item 3).
      InProgress,
      /// This host cannot forget for that agent (yet).
      UnsupportedAgent,
      /// The session recorded no agent home (decision 11).
      NoRecordedHome,
      /// The host's registry has no such (agent, id, home) (B1).
      UnknownToHost,
      /// Another kept session refers to the same agent session (B8).
      Shared,
      /// The root failed a check: `/`, `$HOME` or an ancestor, the host's data
      /// directory, not canonical, not the host user's, writable by others.
      UnsafeRoot,
      /// The root is not there (any more).
      RootMissing,
      /// A symlink, reported and never followed or removed (decision 8).
      Symlink,
      /// A kind directory that is not a real directory.
      NotADirectory,
      /// A kind directory not the host user's, or writable by others (B3).
      UnsafeDirectory,
      /// The walk reached another file system (R2).
      MountPoint,
      /// The walk reached its depth bound (R2).
      TooDeep,
      /// The forget's deadline passed before the removal was done (B6).
      TimedOut,
      /// Still there after the removal (B4).
      StillPresent,
      /// The removal failed midway (B3).
      IoError,
      /// The collector's own: the host answered `error{invalid}` for the id
      /// (decision 8, O10).
      InvalidId,
      /// The collector's own: the host was revoked and never connects again
      /// (O10).
      HostRevoked,
  }

  /// One kind of entry and how many of them (plan 9d B2).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct ForgetWhat {
      pub kind: ForgetKind,
      pub count: u32,
  }

  /// Something a forget left (plan 9d decision 4). `retry: false` marks what
  /// a retry cannot change.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct ForgetRemaining {
      pub what: ForgetWhat,
      pub reason: ForgetReason,
      pub retry: bool,
  }

  /// How a forget ended (plan 9d decision 4): `complete` when the check
  /// afterwards found nothing named left (B4).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum ForgetOutcome {
      Complete,
      Partial,
  }
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
          #[serde(default)]
          indexed: Indexed,
      },
      /// The host accepted a start but could not create the adapter session
  ```

  with:

  ```rust
          #[serde(default)]
          indexed: Indexed,
          /// Where the agent keeps its data for this session (plan 9d
          /// decision 1), registered on the host before this was sent (B1).
          /// Absent from an older host, for an agent hennery cannot forget
          /// for, or when the host could not resolve or register it.
          #[serde(default, skip_serializing_if = "Option::is_none")]
          #[ts(type = "AgentHome | undefined", optional)]
          agent_home: Option<AgentHome>,
      },
      /// The host accepted a start but could not create the adapter session
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              indexed: Indexed::default(),
          }
  ```

  with:

  ```rust
              indexed: Indexed::default(),
              agent_home: None,
          }
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      },
      /// The answer to `browse_directory` (ACP core §3.3, §7): the
  ```

  with:

  ```rust
      },
      /// The answer to `forget_session` (plan 9d decision 4): what was
      /// removed and what is left, as kinds and counts (B2). Like a probe's
      /// reply it is not outboxed and answers only the connection it was
      /// asked on: a forget is idempotent, so a lost answer costs a retry.
      SessionForgotten {
          request_id: String,
          outcome: ForgetOutcome,
          removed: Vec<ForgetWhat>,
          remaining: Vec<ForgetRemaining>,
      },
      /// The answer to `browse_directory` (ACP core §3.3, §7): the
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              Self::ResolvePath { .. } => Ok(Some(Capability::ResolvePath)),
              Self::HelloAck { .. }
  ```

  with:

  ```rust
              Self::ResolvePath { .. } => Ok(Some(Capability::ResolvePath)),
              // Not a probe of state, but carried as one (`session_forgotten`).
              Self::ForgetSession { .. } => Ok(Some(Capability::ForgetSession)),
              Self::HelloAck { .. }
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              | Self::ResolvedPath { request_id, .. } => Some(request_id),
  ```

  with:

  ```rust
              | Self::ResolvedPath { request_id, .. }
              | Self::SessionForgotten { request_id, .. } => Some(request_id),
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
          hat_id: String,
      },
  }
  ```

  with:

  ```rust
          hat_id: String,
      },
      /// Remove a deleted session's transcript from the agent's own data on
      /// the host (plan 9d decision 4): only to a host with the
      /// `forget_session` capability. The host acts only on an exact match
      /// of its own registry (B1). Answered by `session_forgotten` |
      /// `error{invalid}`.
      ForgetSession {
          request_id: String,
          agent: String,
          agent_session_id: String,
          agent_home: AgentHome,
      },
  }
  ```

  In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
  use crate::frames::{ConfigValue, ElicitationAction, PendingKind, PendingReason, SessionConfig};
  ```

  with:

  ```rust
  use crate::frames::{
      ConfigValue, ElicitationAction, ForgetKind, ForgetReason, PendingKind, PendingReason, SessionConfig,
  };
  ```

  In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
              .field("internal_network", &self.internal_network)
              .finish_non_exhaustive()
      }
  }
  ```

  with:

  ```rust
              .field("internal_network", &self.internal_network)
              .finish_non_exhaustive()
      }
  }

  /// Where removing the agent's own transcript of a deleted session stands
  /// (plan 9d decision 7).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum RemovalState {
      /// Its host removed everything it names.
      Removed,
      /// Something is left (`remaining`), or could not be looked for.
      Partial,
      /// Not done yet (`pending`): retried when the host reconnects.
      Pending,
      /// The session had no agent record: nothing on the host.
      None,
  }

  /// Why a removal is still pending (plan 9d decision 7).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum RemovalPending {
      HostOffline,
      /// The host does not announce `forget_session`.
      HostNeedsUpdate,
      /// No answer within the wait (30 s).
      NoReply,
      /// The host still has the agent's session attached.
      Attached,
      /// Another attempt is running right now.
      InProgress,
  }

  /// One kind of entry left on the host, how many, and why (plan 9d B2). Its
  /// path is `kind`'s masked one (`ForgetKind::masked`).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct RemovalItem {
      pub kind: ForgetKind,
      pub count: u32,
      pub reason: ForgetReason,
  }

  /// The agent's own transcript of a deleted session on its host (plan 9d
  /// decision 7): best effort. `notes` name what is never removed, whatever
  /// the state.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct TranscriptRemoval {
      pub state: RemovalState,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "RemovalPending | undefined", optional)]
      pub pending: Option<RemovalPending>,
      pub remaining: Vec<RemovalItem>,
      pub notes: Vec<String>,
  }

  /// `DELETE /api/sessions/{id}` (ACP core §4.10; plan 9d decision 7).
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct DeleteResult {
      pub host_transcript: TranscriptRemoval,
  }

  /// Whether a host removal is still retried (plan 9d decision 6, O10).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum HostRemovalState {
      /// Retried at the host's next handshake.
      Pending,
      /// Done with something left that no retry changes: listed until it is
      /// dismissed.
      Final,
  }

  /// One entry of `GET /api/settings/host-removals` (plan 9d decision 7).
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct HostRemovalItem {
      pub id: String,
      pub host_id: String,
      /// The deleted session (a tombstone).
      pub session_id: String,
      pub agent: String,
      pub state: HostRemovalState,
      pub attempts: u32,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "TranscriptRemoval | undefined", optional)]
      pub last_result: Option<TranscriptRemoval>,
      pub created_at: String,
  }
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      AnswerSubmission, Cursor, Deletion, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, Reassign,
      ResumeRequest, SessionRow, Store, Unattached,
  ```

  with:

  ```rust
      AnswerSubmission, Cursor, Deletion, HostForgets, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery,
      Reassign, ResumeRequest, SessionRow, Store, Unattached,
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      AGENT_MAX_JSON_BYTES, AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, EventDto,
      LifecycleResponse, OpenTurn, PendingItem, PromptRequest, PromptResponse, SessionDetail, StartSessionRequest,
      StartSessionResponse, UpdateSessionRequest, json_width,
  ```

  with:

  ```rust
      AGENT_MAX_JSON_BYTES, AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, DeleteResult,
      EventDto, HostRemovalItem, LifecycleResponse, OpenTurn, PendingItem, PromptRequest, PromptResponse, SessionDetail,
      StartSessionRequest, StartSessionResponse, UpdateSessionRequest, json_width,
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          .route("/api/settings/attachments", get(attachment_usage));
  ```

  with:

  ```rust
          .route("/api/settings/attachments", get(attachment_usage))
          .route("/api/settings/host-removals", get(host_removals))
          .route(
              "/api/settings/host-removals/{id}",
              axum::routing::delete(
                  dismiss_host_removal.layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
              ),
          );
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
  /// lifecycle if something moved it on meanwhile (a resume). 204.
  ```

  with:

  ```rust
  /// lifecycle if something moved it on meanwhile (a resume). 200 with what
  /// became of the agent's own transcript on the host (plan 9d decision 7).
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      finish_delete(&state, &session, unattached.as_ref())
  }

  /// The delete itself, once `close_through_host` has judged `session`.
  pub(crate) fn finish_delete(state: &AppState, session: &SessionRow, unattached: Option<&Unattached>) -> Response {
  ```

  with:

  ```rust
      let forgets = match finish_delete(&state, &session, unattached.as_ref()) {
          Ok(forgets) => forgets,
          Err(response) => return *response,
      };
      // The agent's own transcript on the host, best effort (plan 9d
      // decisions 5 and 7): what the host answers within the wait, or why it
      // is still pending. A purge's `PurgeResult` gets these as counts when
      // plan 9c rebases onto this (plan 9d decision 7): 9c hand-off.
      let host_transcript = crate::forget::after_delete(&state, &forgets).await;
      (StatusCode::OK, Json(DeleteResult { host_transcript })).into_response()
  }

  /// The delete itself, once `close_through_host` has judged `session`: what
  /// it left for the host to remove (plan 9d decision 2), or the answer.
  pub(crate) fn finish_delete(
      state: &AppState,
      session: &SessionRow,
      unattached: Option<&Unattached>,
  ) -> Result<HostForgets, Box<Response>> {
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          Ok(Deletion::Done { event, unconfirmed }) => {
  ```

  with:

  ```rust
          Ok(Deletion::Done {
              event,
              unconfirmed,
              forgets,
          }) => {
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
              StatusCode::NO_CONTENT.into_response()
          }
          Ok(Deletion::Refused(lifecycle)) => error(
              StatusCode::CONFLICT,
              &lifecycle,
              format!("the session is {lifecycle} now; delete it again once that settles"),
          ),
          Ok(Deletion::NotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
  ```

  with:

  ```rust
              Ok(*forgets)
          }
          Ok(Deletion::Refused(lifecycle)) => Err(Box::new(error(
              StatusCode::CONFLICT,
              &lifecycle,
              format!("the session is {lifecycle} now; delete it again once that settles"),
          ))),
          Ok(Deletion::NotFound) => Err(Box::new(error(StatusCode::NOT_FOUND, "not_found", "no such session"))),
          Err(err) => Err(Box::new(internal(err))),
      }
  }

  /// `GET /api/settings/host-removals` (plan 9d decision 7): what deletes
  /// left to remove on hosts, pending or final. Read-only, so no step-up.
  async fn host_removals(State(state): State<AppState>) -> Response {
      match state.store.host_removals() {
          Ok(records) => {
              let items: Vec<HostRemovalItem> = records.into_iter().map(crate::forget::listed).collect();
              Json(items).into_response()
          }
          Err(err) => internal(err),
      }
  }

  /// `DELETE /api/settings/host-removals/{id}` (plan 9d O10), behind step-up:
  /// dismiss one, which is then neither retried nor listed. 204, or 404.
  async fn dismiss_host_removal(State(state): State<AppState>, Path(id): Path<String>) -> Response {
      match state.store.dismiss_forget(&id) {
          Ok(true) => StatusCode::NO_CONTENT.into_response(),
          Ok(false) => error(StatusCode::NOT_FOUND, "not_found", "no such host removal"),
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          let judged = judged_unattached(&f, &s).await;
          let response = finish_delete(&f.state, &s, Some(&judged));
          assert_eq!(response.status(), StatusCode::NO_CONTENT);
          assert!(
  ```

  with:

  ```rust
          let judged = judged_unattached(&f, &s).await;
          assert!(finish_delete(&f.state, &s, Some(&judged)).is_ok());
          assert!(
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          let response = finish_delete(&f.state, &s, Some(&judged));
          assert_eq!(response.status(), StatusCode::NO_CONTENT);
  ```

  with:

  ```rust
          assert!(finish_delete(&f.state, &s, Some(&judged)).is_ok());
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          assert_eq!(
              finish_delete(&f.state, &s, Some(&judged)).status(),
              StatusCode::NO_CONTENT
          );
  ```

  with:

  ```rust
          assert!(finish_delete(&f.state, &s, Some(&judged)).is_ok());
  ```

  Create `crates/hennery-sessions/src/forget.rs`:

  ```rust
  //! The agent's own transcript of a deleted session, on its host (plan 9d
  //! decisions 4–7): sending `forget_session` for the records a delete left
  //! (`store::HostForgets`), and what the answers come to.
  //!
  //! A record goes right after its delete commits (the route waits at most
  //! `FORGET_WAIT` for all of them), after each reconciled handshake of its
  //! host (`retry_host`), and after its session's `session_closed`
  //! (`retry_session`, O10). One attempt per record is in flight at a time
  //! (B7, `InFlight`). The host's answer is kinds and counts with fixed
  //! reasons (B2): nothing in it is a path, and it is checked before it is
  //! stored. What a host reports is not verified beyond that.

  use crate::AppState;
  use crate::hub::RequestError;
  use crate::store::{ForgetRecord, HostForgets, final_result};
  use hennery_proto::frames::{CollectorFrame, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
  use hennery_proto::rest::{HostRemovalItem, RemovalItem, RemovalPending, RemovalState, TranscriptRemoval};
  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;

  /// How long a delete waits for its host's answers, all records together
  /// (plan 9d decision 5), and how long one attempt waits after a handshake.
  /// Above the host's own deadline (`hennery_host::forget::FORGET_DEADLINE`),
  /// so a live host's answer comes first.
  pub const FORGET_WAIT: Duration = Duration::from_secs(30);

  /// The most entries of each list an answer may hold (B2's size cap): a
  /// forget names at most a handful of kinds.
  pub const MAX_ANSWER_ITEMS: usize = 16;

  /// What is never removed for a Claude session, whatever the outcome (plan
  /// 9d's known limitation, O12, B9). Always in a Claude session's result.
  pub const CLAUDE_NOTES: [&str; 3] = [
      "transcripts started after a context clear inside the agent are not removed",
      "the agent's history.jsonl, telemetry/1p_failed_events.* and todos/ may still name this session",
      "only the transcript's own names in each project directory are removed; anything else there is left",
  ];

  /// The notes a session of `agent` always gets (plan 9d decision 7).
  pub fn notes(agent: &str) -> Vec<String> {
      match agent {
          "claude" => CLAUDE_NOTES.iter().map(|n| n.to_string()).collect(),
          _ => Vec::new(),
      }
  }

  /// The records with an attempt in flight (B7): one at a time each. Each
  /// says whether another attempt was asked for meanwhile (the review's
  /// item 11): the holder then goes again, so a `session_closed` that lands
  /// while an `attached` answer is on its way is not lost.
  #[derive(Default)]
  pub struct InFlight(Mutex<HashMap<String, bool>>);

  impl InFlight {
      /// Take `id`, unless an attempt holds it, in which case that attempt is
      /// asked to go again. Released when the claim is dropped, however the
      /// attempt ends (its HTTP client gone included).
      fn claim(self: &Arc<Self>, id: &str) -> Option<Claim> {
          let mut held = self.0.lock().expect("in-flight lock");
          if let Some(again) = held.get_mut(id) {
              *again = true;
              return None;
          }
          held.insert(id.to_string(), false);
          Some(Claim {
              set: self.clone(),
              id: id.to_string(),
          })
      }
  }

  struct Claim {
      set: Arc<InFlight>,
      id: String,
  }

  impl Claim {
      /// Whether another attempt was asked for since the claim was taken (or
      /// last asked), clearing the request.
      fn asked_again(&self) -> bool {
          self.set
              .0
              .lock()
              .expect("in-flight lock")
              .get_mut(&self.id)
              .map(std::mem::take)
              .unwrap_or(false)
      }
  }

  impl Drop for Claim {
      fn drop(&mut self) {
          self.set.0.lock().expect("in-flight lock").remove(&self.id);
      }
  }

  fn pending(why: RemovalPending) -> TranscriptRemoval {
      TranscriptRemoval {
          state: RemovalState::Pending,
          pending: Some(why),
          remaining: Vec::new(),
          notes: Vec::new(),
      }
  }

  /// An answer's lists within the caps (B2).
  fn well_formed(removed: &[ForgetWhat], remaining: &[ForgetRemaining]) -> bool {
      removed.len() <= MAX_ANSWER_ITEMS && remaining.len() <= MAX_ANSWER_ITEMS
  }

  /// What a host's answer comes to, and whether the record is done with
  /// (plan 9d decision 6): `complete`, or `partial` with nothing a retry can
  /// change. A partial answer whose only retryable reason is `attached` is
  /// pending for that reason (decision 7).
  pub fn answered(outcome: ForgetOutcome, remaining: &[ForgetRemaining]) -> (TranscriptRemoval, bool) {
      if outcome == ForgetOutcome::Complete && remaining.is_empty() {
          let removed = TranscriptRemoval {
              state: RemovalState::Removed,
              pending: None,
              remaining: Vec::new(),
              notes: Vec::new(),
          };
          return (removed, true);
      }
      let items: Vec<RemovalItem> = remaining
          .iter()
          .map(|r| RemovalItem {
              kind: r.what.kind,
              count: r.what.count,
              reason: r.reason,
          })
          .collect();
      let retryable: Vec<&ForgetRemaining> = remaining.iter().filter(|r| r.retry).collect();
      // Waiting on the host only: its adapter (attached), or another forget
      // of the same agent session running there (in progress).
      let waiting = !retryable.is_empty()
          && retryable
              .iter()
              .all(|r| matches!(r.reason, ForgetReason::Attached | ForgetReason::InProgress));
      let why = if retryable.iter().any(|r| r.reason == ForgetReason::Attached) {
          RemovalPending::Attached
      } else {
          RemovalPending::InProgress
      };
      let result = TranscriptRemoval {
          state: if waiting {
              RemovalState::Pending
          } else {
              RemovalState::Partial
          },
          pending: waiting.then_some(why),
          remaining: items,
          notes: Vec::new(),
      };
      (result, retryable.is_empty())
  }

  /// One attempt at `record`, waiting at most `wait` for the host: its result,
  /// stored unless another attempt holds the record (`in_progress`, nothing
  /// stored; the holder goes again once it is done, within its own wait).
  pub async fn attempt(state: &AppState, record: &ForgetRecord, wait: Duration) -> TranscriptRemoval {
      let Some(claim) = state.forgets.claim(&record.id) else {
          return pending(RemovalPending::InProgress);
      };
      let until = tokio::time::Instant::now() + wait;
      loop {
          let (result, done) = attempt_once(
              state,
              record,
              until.saturating_duration_since(tokio::time::Instant::now()),
          )
          .await;
          if done || !claim.asked_again() || tokio::time::Instant::now() >= until {
              return result;
          }
      }
  }

  /// One `forget_session` for `record`, under its claim: the result and
  /// whether the record is done with.
  async fn attempt_once(state: &AppState, record: &ForgetRecord, wait: Duration) -> (TranscriptRemoval, bool) {
      let Some(home) = record.agent_home.clone() else {
          let known = record
              .last_result
              .clone()
              .unwrap_or_else(|| final_result(ForgetReason::NoRecordedHome));
          return (known, true);
      };
      // Another kept session may have taken up the agent session since the
      // delete (a resume of a session that shares it): not forgotten then,
      // final (B8, the review's item 13).
      match state.store.forget_is_shared(record) {
          Ok(false) => {}
          Ok(true) => {
              let result = final_result(ForgetReason::Shared);
              if let Err(err) = state.store.forget_attempted(&record.id, &result, false, true) {
                  tracing::error!(host_id = %record.host_id, error = %err, "recording a forget's result failed");
              }
              return (result, true);
          }
          Err(err) => {
              tracing::error!(host_id = %record.host_id, error = %err, "checking whether a forget is shared failed");
              return (pending(RemovalPending::NoReply), false);
          }
      }
      let request_id = uuid::Uuid::now_v7().to_string();
      let frame = CollectorFrame::ForgetSession {
          request_id: request_id.clone(),
          agent: record.agent.clone(),
          agent_session_id: record.agent_session_id.clone(),
          agent_home: home,
      };
      let (result, sent, done) = match state.hub.probe(&record.host_id, &request_id, frame, wait).await {
          Ok(HostFrame::SessionForgotten {
              outcome,
              removed,
              remaining,
              ..
          }) if well_formed(&removed, &remaining) => {
              let (result, done) = answered(outcome, &remaining);
              (result, true, done)
          }
          Ok(_) => {
              tracing::warn!(host_id = %record.host_id, "a forget's answer is out of bounds; it stays pending");
              (pending(RemovalPending::NoReply), true, false)
          }
          Err(RequestError::NotConnected) => (pending(RemovalPending::HostOffline), false, false),
          Err(RequestError::Unsupported) => (pending(RemovalPending::HostNeedsUpdate), false, false),
          Err(RequestError::Busy) => (pending(RemovalPending::NoReply), false, false),
          Err(RequestError::DeliveryUnknown) => (pending(RemovalPending::NoReply), true, false),
          // The id is not one the agent writes (decision 8): no retry
          // changes it (O10).
          Err(RequestError::Rejected { code, .. }) if code == "invalid" => {
              (final_result(ForgetReason::InvalidId), true, true)
          }
          Err(RequestError::Rejected { code, message }) => {
              tracing::warn!(host_id = %record.host_id, %code, %message, "a forget was refused; it stays pending");
              (pending(RemovalPending::NoReply), true, false)
          }
      };
      if let Err(err) = state.store.forget_attempted(&record.id, &result, sent, done) {
          tracing::error!(host_id = %record.host_id, error = %err, "recording a forget's result failed");
      }
      (result, done)
  }

  /// Right after a delete committed (plan 9d decision 5): each of its pending
  /// records, while `FORGET_WAIT` lasts, all together; then the result for
  /// the delete's answer (decision 7).
  pub async fn after_delete(state: &AppState, forgets: &HostForgets) -> TranscriptRemoval {
      let deadline = tokio::time::Instant::now() + FORGET_WAIT;
      let mut results = Vec::new();
      for record in &forgets.records {
          let result = match &record.last_result {
              // Known without the host (no home, a revoked host).
              Some(known) => known.clone(),
              None => {
                  let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                  if left.is_zero() {
                      pending(RemovalPending::NoReply)
                  } else {
                      attempt(state, record, left).await
                  }
              }
          };
          results.push(result);
      }
      if forgets.shared > 0 {
          results.push(final_result(ForgetReason::Shared));
      }
      combined(forgets.had_agent_record, &forgets.agent, results)
  }

  /// The results of a session's records as one (decision 7): pending if any
  /// is, else partial if any is, else removed; `none` with no agent record.
  pub fn combined(had_agent_record: bool, agent: &str, results: Vec<TranscriptRemoval>) -> TranscriptRemoval {
      let notes = notes(agent);
      if !had_agent_record {
          return TranscriptRemoval {
              state: RemovalState::None,
              pending: None,
              remaining: Vec::new(),
              notes,
          };
      }
      let state = if results.iter().any(|r| r.state == RemovalState::Pending) {
          RemovalState::Pending
      } else if results.iter().any(|r| r.state == RemovalState::Partial) {
          RemovalState::Partial
      } else {
          RemovalState::Removed
      };
      TranscriptRemoval {
          state,
          pending: results.iter().find_map(|r| r.pending),
          remaining: results.into_iter().flat_map(|r| r.remaining).collect(),
          notes,
      }
  }

  /// A record as `GET /api/settings/host-removals` lists it.
  pub fn listed(record: ForgetRecord) -> HostRemovalItem {
      let notes = notes(&record.agent);
      HostRemovalItem {
          id: record.id,
          host_id: record.host_id,
          session_id: record.session_id,
          state: record.state,
          attempts: record.attempts,
          last_result: record.last_result.map(|mut result| {
              result.notes = notes;
              result
          }),
          agent: record.agent,
          created_at: record.created_at,
      }
  }

  /// After a reconciled handshake (plan 9d decision 5): the host's pending
  /// records, one at a time, while it stays ready.
  pub fn retry_host(state: &AppState, host_id: &str) {
      let (state, host_id) = (state.clone(), host_id.to_string());
      tokio::spawn(async move {
          let records = match state.store.forgets_to_send(&host_id) {
              Ok(records) => records,
              Err(err) => {
                  tracing::warn!(%host_id, error = %err, "reading the host's pending removals failed");
                  return;
              }
          };
          for record in records {
              if !state.hub.is_ready(&host_id) {
                  return;
              }
              attempt(&state, &record, FORGET_WAIT).await;
          }
      });
  }

  /// After a deleted session's `session_closed` (plan 9d O10): its records,
  /// which an `attached` answer left pending.
  pub fn retry_session(state: &AppState, session_id: &str) {
      let (state, session_id) = (state.clone(), session_id.to_string());
      tokio::spawn(async move {
          let records = match state.store.forgets_of_session(&session_id) {
              Ok(records) => records,
              Err(err) => {
                  tracing::warn!(%session_id, error = %err, "reading the session's pending removals failed");
                  return;
              }
          };
          for record in records {
              attempt(&state, &record, FORGET_WAIT).await;
          }
      });
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use hennery_proto::frames::ForgetKind;

      fn left(kind: ForgetKind, reason: ForgetReason, retry: bool) -> ForgetRemaining {
          ForgetRemaining {
              what: ForgetWhat { kind, count: 1 },
              reason,
              retry,
          }
      }

      #[test]
      fn a_record_is_done_when_complete_or_when_no_retry_can_change_what_is_left() {
          let (result, done) = answered(ForgetOutcome::Complete, &[]);
          assert_eq!((result.state, done), (RemovalState::Removed, true));
          let symlink = left(ForgetKind::FileHistory, ForgetReason::Symlink, false);
          let (result, done) = answered(ForgetOutcome::Partial, &[symlink]);
          assert_eq!((result.state, done), (RemovalState::Partial, true));
          let io = left(ForgetKind::Tasks, ForgetReason::IoError, true);
          let (result, done) = answered(ForgetOutcome::Partial, &[symlink, io]);
          assert_eq!((result.state, done), (RemovalState::Partial, false));
          let attached = left(ForgetKind::Session, ForgetReason::Attached, true);
          let (result, done) = answered(ForgetOutcome::Partial, &[attached]);
          assert_eq!(
              (result.state, result.pending, done),
              (RemovalState::Pending, Some(RemovalPending::Attached), false)
          );
          let busy = left(ForgetKind::Session, ForgetReason::InProgress, true);
          let (result, done) = answered(ForgetOutcome::Partial, &[busy]);
          assert_eq!(
              (result.state, result.pending, done),
              (RemovalState::Pending, Some(RemovalPending::InProgress), false)
          );
      }

      #[test]
      fn a_sessions_results_combine_to_the_worst_and_claude_always_has_its_notes() {
          let removed = answered(ForgetOutcome::Complete, &[]).0;
          let all = combined(true, "claude", vec![removed.clone(), removed.clone()]);
          assert_eq!(all.state, RemovalState::Removed);
          assert_eq!(all.notes.len(), CLAUDE_NOTES.len());
          assert!(all.notes[0].contains("context clear"));
          let mixed = combined(true, "codex", vec![removed, final_result(ForgetReason::Shared)]);
          assert_eq!(mixed.state, RemovalState::Partial);
          assert!(mixed.notes.is_empty());
          let none = combined(false, "claude", Vec::new());
          assert_eq!(none.state, RemovalState::None);
          assert_eq!(none.notes.len(), CLAUDE_NOTES.len());
          let waiting = combined(
              true,
              "claude",
              vec![pending(RemovalPending::HostOffline), final_result(ForgetReason::Shared)],
          );
          assert_eq!(
              (waiting.state, waiting.pending, waiting.remaining.len()),
              (RemovalState::Pending, Some(RemovalPending::HostOffline), 1)
          );
      }

      #[test]
      fn one_attempt_at_a_time_per_record() {
          let set = Arc::new(InFlight::default());
          let first = set.claim("f1").expect("free");
          assert!(!first.asked_again());
          assert!(set.claim("f1").is_none());
          // The refused claim asked the holder to go again, once.
          assert!(first.asked_again());
          assert!(!first.asked_again());
          assert!(set.claim("f2").is_some());
          drop(first);
          assert!(set.claim("f1").is_some());
      }
  }
  ```

  In `crates/hennery-sessions/src/hats.rs`, replace:

  ```rust
          Deletion::Done { event, unconfirmed } => {
  ```

  with:

  ```rust
          Deletion::Done { event, unconfirmed, .. } => {
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  pub mod content;
  pub mod hats;
  ```

  with:

  ```rust
  pub mod content;
  pub mod forget;
  pub mod hats;
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      pub sweep_interval: Duration,
  }
  ```

  with:

  ```rust
      pub sweep_interval: Duration,
      /// The host removals with an attempt in flight (plan 9d B7).
      pub forgets: Arc<forget::InFlight>,
  }
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
              sweep_interval: sweep::INTERVAL,
          }
  ```

  with:

  ```rust
              sweep_interval: sweep::INTERVAL,
              forgets: Arc::new(forget::InFlight::default()),
          }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      AttachedSession, CollectorFrame, ConfigValue, ElicitationAction, Indexed, ParkReason, PendingKind, PendingReason,
      PendingResolution, SessionBody, SessionConfig, TurnOutcome,
  };
  use hennery_proto::rest::{
      AnswerRequest, AttachmentUsage, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, EventDto, PendingItem, PendingState,
      SessionCatalog, SessionItem, SessionPage, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES, json_char_width,
  ```

  with:

  ```rust
      AgentHome, AttachedSession, CollectorFrame, ConfigValue, ElicitationAction, ForgetKind, ForgetReason, Indexed,
      ParkReason, PendingKind, PendingReason, PendingResolution, SessionBody, SessionConfig, TurnOutcome,
  };
  use hennery_proto::rest::{
      AnswerRequest, AttachmentUsage, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, EventDto, HostRemovalState, PendingItem,
      PendingState, RemovalItem, RemovalState, SessionCatalog, SessionItem, SessionPage, TITLE_MAX_CHARS,
      TITLE_MAX_JSON_BYTES, TranscriptRemoval, json_char_width,
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  ",
  ];

  ```

  with:

  ```rust
  ",
      // The agent's own transcript on its host (plan 9d decisions 1–3, B8).
      // `sessions.agent_home`: every distinct (agent session id, roots) the
      // session's host reported in a `session_started`, at most
      // `MAX_AGENT_HOMES`, as a JSON array; NULL before 9d, and on a
      // tombstone. `host_forgets`: what a delete left to remove on a host,
      // one row per pair, written in the delete's own transaction. It keeps
      // the agent's ids and roots, never a cwd, a title or any content, and
      // nothing reads it as a session (decision 3). `state` is `pending`
      // (retried at the host's next handshake) or `final` (something is
      // left that no retry changes; listed until dismissed, O10); a removal
      // the host reports complete deletes its row. No backfill: a session
      // from before has no home (decision 11).
      "
      ALTER TABLE sessions ADD COLUMN agent_home TEXT;
      CREATE TABLE host_forgets (
          id TEXT PRIMARY KEY,
          owner_id TEXT NOT NULL REFERENCES owners(id),
          host_id TEXT NOT NULL,
          session_id TEXT NOT NULL REFERENCES sessions(id),
          hat_id TEXT NOT NULL,
          agent TEXT NOT NULL,
          agent_session_id TEXT NOT NULL,
          agent_home TEXT,
          created_at TEXT NOT NULL,
          attempts INTEGER NOT NULL DEFAULT 0,
          last_result TEXT,
          state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'final')));
      CREATE INDEX host_forgets_by_host ON host_forgets(owner_id, host_id, state);
      CREATE INDEX host_forgets_by_session ON host_forgets(owner_id, session_id);
  ",
  ];

  /// The most distinct (agent session id, roots) pairs a session records
  /// (plan 9d B8). A pair past it is not recorded, and so not forgotten.
  pub const MAX_AGENT_HOMES: usize = 8;

  /// One (agent session id, roots) a session used (plan 9d B8), as
  /// `sessions.agent_home` keeps it.
  #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
  pub struct RecordedHome {
      pub agent_session_id: String,
      #[serde(flatten)]
      pub home: AgentHome,
  }

  /// A removal still to make, or made with something left, on a host (plan
  /// 9d decisions 2 and 6): one `host_forgets` row.
  #[derive(Debug, Clone, PartialEq)]
  pub struct ForgetRecord {
      pub id: String,
      pub host_id: String,
      pub session_id: String,
      pub agent: String,
      pub agent_session_id: String,
      /// `None`: the session recorded none (decision 11), and the record is
      /// final from the start.
      pub agent_home: Option<AgentHome>,
      pub state: HostRemovalState,
      pub attempts: u32,
      pub last_result: Option<TranscriptRemoval>,
      pub created_at: String,
  }

  /// What a delete left for the session's host (plan 9d decision 2, B8).
  #[derive(Debug, Clone, Default, PartialEq)]
  pub struct HostForgets {
      /// The session's agent, as it was.
      pub agent: String,
      /// It had an agent session id: there is something on the host.
      pub had_agent_record: bool,
      /// One per recorded pair, as written.
      pub records: Vec<ForgetRecord>,
      /// Pairs another kept session still refers to: not forgotten (B8).
      pub shared: u32,
  }

  const FORGET_COLUMNS: &str =
      "id, host_id, session_id, agent, agent_session_id, agent_home, state, attempts, last_result, created_at";

  fn read_forget(r: &rusqlite::Row<'_>) -> rusqlite::Result<(ForgetRecord, Option<String>, String, Option<String>)> {
      Ok((
          ForgetRecord {
              id: r.get(0)?,
              host_id: r.get(1)?,
              session_id: r.get(2)?,
              agent: r.get(3)?,
              agent_session_id: r.get(4)?,
              agent_home: None,
              state: HostRemovalState::Pending,
              attempts: r.get(7)?,
              last_result: None,
              created_at: r.get(9)?,
          },
          r.get(5)?,
          r.get(6)?,
          r.get(8)?,
      ))
  }

  /// A `read_forget` row with its JSON and state decoded.
  fn decode_forget(
      (mut record, home, state, result): (ForgetRecord, Option<String>, String, Option<String>),
  ) -> Result<ForgetRecord> {
      record.agent_home = home.map(|h| serde_json::from_str(&h)).transpose()?;
      record.state = if state == "final" {
          HostRemovalState::Final
      } else {
          HostRemovalState::Pending
      };
      record.last_result = result.map(|r| serde_json::from_str(&r)).transpose()?;
      Ok(record)
  }

  /// The pairs `sessions.agent_home` holds; an unreadable value counts as
  /// none, logged (it is only ever written by `record_home`).
  fn recorded_homes(raw: Option<&str>) -> Vec<RecordedHome> {
      raw.map(|raw| {
          serde_json::from_str(raw).unwrap_or_else(|err| {
              tracing::warn!(error = %err, "a session's recorded agent homes do not parse");
              Vec::new()
          })
      })
      .unwrap_or_default()
  }

  /// Add the (agent session id, home) a `session_started` reported to the
  /// session's recorded pairs (plan 9d B8): only a well-formed home (the
  /// shape check), only a pair not there yet, at most `MAX_AGENT_HOMES`.
  fn record_home(
      tx: &Transaction<'_>,
      owner: &str,
      session_id: &str,
      agent_session_id: &str,
      home: &AgentHome,
  ) -> Result<()> {
      if !home.is_well_formed() {
          tracing::warn!(%session_id, "not recording an agent home that is not absolute, bounded and free of NUL");
          return Ok(());
      }
      let raw: Option<String> = tx.query_row(
          "SELECT agent_home FROM sessions WHERE id = ?1 AND owner_id = ?2",
          [session_id, owner],
          |r| r.get(0),
      )?;
      let mut homes = recorded_homes(raw.as_deref());
      let pair = RecordedHome {
          agent_session_id: agent_session_id.to_string(),
          home: home.clone(),
      };
      if homes.contains(&pair) {
          return Ok(());
      }
      if homes.len() >= MAX_AGENT_HOMES {
          tracing::warn!(%session_id, "a session reported more agent homes than are recorded; this one is not");
          return Ok(());
      }
      homes.push(pair);
      tx.execute(
          "UPDATE sessions SET agent_home = ?2 WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?3",
          params![session_id, serde_json::to_string(&homes)?, owner],
      )?;
      Ok(())
  }

  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      Done { event: EventDto, unconfirmed: bool },
  ```

  with:

  ```rust
      Done {
          event: EventDto,
          unconfirmed: bool,
          /// What is left to remove on its host (plan 9d decision 2).
          forgets: Box<HostForgets>,
      },
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust

  impl Store {
  ```

  with:

  ```rust

  /// The deleted session, as `write_forgets` reads it.
  struct ForgetSource<'a> {
      session_id: &'a str,
      host_id: &'a str,
      hat_id: &'a str,
      agent: &'a str,
      agent_session_id: Option<&'a str>,
      agent_home: Option<&'a str>,
  }

  /// A record's result when the collector knows it without asking the host:
  /// final, with `reason` for the whole session (plan 9d decision 11, O10).
  pub fn final_result(reason: ForgetReason) -> TranscriptRemoval {
      TranscriptRemoval {
          state: RemovalState::Partial,
          pending: None,
          remaining: vec![RemovalItem {
              kind: ForgetKind::Session,
              count: 0,
              reason,
          }],
          notes: Vec::new(),
      }
  }

  /// In the delete's transaction, before the scrub (plan 9d decision 2):
  /// one `host_forgets` row per (agent session id, roots) the session used
  /// (B8), and one with no home for an agent session id it recorded none
  /// for (a session from before 9d: final, decision 11). None for a pair
  /// another kept session of that host and agent still refers to, by its
  /// agent session id or among its recorded pairs (B8, `shared`). A record
  /// for a revoked host is final (O10): it never connects again.
  fn write_forgets(tx: &Transaction<'_>, owner: &str, deleted: ForgetSource<'_>) -> Result<HostForgets> {
      let mut pairs: Vec<(String, Option<AgentHome>)> = recorded_homes(deleted.agent_home)
          .into_iter()
          .map(|recorded| (recorded.agent_session_id, Some(recorded.home)))
          .collect();
      if let Some(id) = deleted.agent_session_id
          && !pairs.iter().any(|(recorded, _)| recorded == id)
      {
          pairs.push((id.to_string(), None));
      }
      let mut out = HostForgets {
          agent: deleted.agent.to_string(),
          had_agent_record: !pairs.is_empty(),
          ..HostForgets::default()
      };
      let revoked: bool = tx
          .query_row(
              "SELECT revoked_at IS NOT NULL FROM hosts WHERE id = ?1 AND owner_id = ?2",
              [deleted.host_id, owner],
              |r| r.get(0),
          )
          .optional()?
          .unwrap_or(false);
      let in_use = agent_sessions_in_use(tx, owner, deleted.host_id, deleted.agent, deleted.session_id)?;
      let ts = now();
      for (agent_session_id, home) in pairs {
          if in_use.contains(&agent_session_id) {
              out.shared += 1;
              continue;
          }
          let known = match (&home, revoked) {
              (None, _) => Some(final_result(ForgetReason::NoRecordedHome)),
              (Some(_), true) => Some(final_result(ForgetReason::HostRevoked)),
              (Some(_), false) => None,
          };
          let state = if known.is_some() {
              HostRemovalState::Final
          } else {
              HostRemovalState::Pending
          };
          // A final record is never sent: it keeps no roots (the review's
          // item 12).
          let home = if known.is_some() { None } else { home };
          let record = ForgetRecord {
              id: uuid::Uuid::now_v7().to_string(),
              host_id: deleted.host_id.to_string(),
              session_id: deleted.session_id.to_string(),
              agent: deleted.agent.to_string(),
              agent_session_id,
              agent_home: home,
              state,
              attempts: 0,
              last_result: known,
              created_at: ts.clone(),
          };
          tx.execute(
              "INSERT INTO host_forgets(id, owner_id, host_id, session_id, hat_id, agent, agent_session_id, agent_home,
                                        created_at, last_result, state)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
              params![
                  record.id,
                  owner,
                  record.host_id,
                  record.session_id,
                  deleted.hat_id,
                  record.agent,
                  record.agent_session_id,
                  record.agent_home.as_ref().map(serde_json::to_string).transpose()?,
                  record.created_at,
                  record.last_result.as_ref().map(serde_json::to_string).transpose()?,
                  state_name(state),
              ],
          )?;
          out.records.push(record);
      }
      Ok(out)
  }

  /// The agent sessions the kept sessions of `host_id` and `agent` other than
  /// `except` refer to (B8): by their id, and among their recorded pairs.
  fn agent_sessions_in_use(
      conn: &Connection,
      owner: &str,
      host_id: &str,
      agent: &str,
      except: &str,
  ) -> Result<BTreeSet<String>> {
      let mut stmt = conn.prepare(
          "SELECT agent_session_id, agent_home FROM sessions
           WHERE host_id = ?2 AND agent = ?3 AND id <> ?4 AND lifecycle <> 'deleted' AND owner_id = ?1
               AND (agent_session_id IS NOT NULL OR agent_home IS NOT NULL)",
      )?;
      let rows = stmt.query_map(params![owner, host_id, agent, except], |r| {
          Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))
      })?;
      let mut ids = BTreeSet::new();
      for row in rows {
          let (id, homes) = row?;
          ids.extend(id);
          ids.extend(recorded_homes(homes.as_deref()).into_iter().map(|h| h.agent_session_id));
      }
      Ok(ids)
  }

  fn state_name(state: HostRemovalState) -> &'static str {
      match state {
          HostRemovalState::Pending => "pending",
          HostRemovalState::Final => "final",
      }
  }

  impl Store {
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
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
  ```

  with:

  ```rust
          // Its host, hat and cwd are read before the scrub clears them (R1),
          // and so are its agent and what it recorded of the agent's data
          // (plan 9d decision 2, B8).
          type DeletedRow = (
              String,
              bool,
              String,
              String,
              String,
              String,
              Option<String>,
              Option<String>,
          );
          let row: Option<DeletedRow> = tx
              .query_row(
                  "SELECT lifecycle, presumed_parked, host_id, hat_id, cwd, agent, agent_session_id, agent_home
                   FROM sessions WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                  [session_id, &self.owner],
                  |r| {
                      Ok((
                          r.get(0)?,
                          r.get(1)?,
                          r.get(2)?,
                          r.get(3)?,
                          r.get(4)?,
                          r.get(5)?,
                          r.get(6)?,
                          r.get(7)?,
                      ))
                  },
              )
              .optional()?;
          let Some((lifecycle, presumed, host_id, hat_id, cwd, agent, agent_session_id, agent_home)) = row else {
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let dropped = drop_unreferenced(&tx, &self.owner, &hashes)?;
          let event = collector_event(&tx, &self.owner, session_id, "session_deleted", json!({}), &now())?;
  ```

  with:

  ```rust
          let dropped = drop_unreferenced(&tx, &self.owner, &hashes)?;
          // What is left to remove on the host, before the scrub (plan 9d
          // decision 2): one record per pair the session used.
          let forgets = write_forgets(
              &tx,
              &self.owner,
              ForgetSource {
                  session_id,
                  host_id: &host_id,
                  hat_id: &hat_id,
                  agent: &agent,
                  agent_session_id: agent_session_id.as_deref(),
                  agent_home: agent_home.as_deref(),
              },
          )?;
          let event = collector_event(&tx, &self.owner, session_id, "session_deleted", json!({}), &now())?;
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                   open_turn_id = NULL, activity = NULL, presumed_parked = 0, close_requested = 0
  ```

  with:

  ```rust
                   open_turn_id = NULL, activity = NULL, presumed_parked = 0, close_requested = 0, agent_home = NULL
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          Ok(Deletion::Done { event, unconfirmed })
  ```

  with:

  ```rust
          Ok(Deletion::Done {
              event,
              unconfirmed,
              forgets: Box::new(forgets),
          })
      }

      /// A host's records still to send, oldest first (plan 9d decision 5):
      /// pending, with a home.
      pub fn forgets_to_send(&self, host_id: &str) -> Result<Vec<ForgetRecord>> {
          let conn = self.conn();
          let mut stmt = conn.prepare(&format!(
              "SELECT {FORGET_COLUMNS} FROM host_forgets
               WHERE host_id = ?1 AND state = 'pending' AND agent_home IS NOT NULL AND owner_id = ?2
               ORDER BY created_at, id"
          ))?;
          let rows = stmt.query_map([host_id, &self.owner], read_forget)?;
          rows.map(|row| decode_forget(row?)).collect()
      }

      /// A deleted session's records still to send (plan 9d O10: a retry
      /// follows its `session_closed`).
      pub fn forgets_of_session(&self, session_id: &str) -> Result<Vec<ForgetRecord>> {
          let conn = self.conn();
          let mut stmt = conn.prepare(&format!(
              "SELECT {FORGET_COLUMNS} FROM host_forgets
               WHERE session_id = ?1 AND state = 'pending' AND agent_home IS NOT NULL AND owner_id = ?2
               ORDER BY created_at, id"
          ))?;
          let rows = stmt.query_map([session_id, &self.owner], read_forget)?;
          rows.map(|row| decode_forget(row?)).collect()
      }

      /// Every record, oldest first (`GET /api/settings/host-removals`).
      pub fn host_removals(&self) -> Result<Vec<ForgetRecord>> {
          let conn = self.conn();
          let mut stmt = conn.prepare(&format!(
              "SELECT {FORGET_COLUMNS} FROM host_forgets WHERE owner_id = ?1 ORDER BY created_at, id"
          ))?;
          let rows = stmt.query_map([&self.owner], read_forget)?;
          rows.map(|row| decode_forget(row?)).collect()
      }

      /// What one attempt at a record came to (plan 9d decision 6): a
      /// complete removal deletes the record; otherwise its result is kept,
      /// `sent` attempts are counted, and `final` stops the retries. Only a
      /// record still pending changes: a dismissed or final one is left.
      pub fn forget_attempted(&self, id: &str, result: &TranscriptRemoval, sent: bool, done: bool) -> Result<()> {
          let conn = self.conn();
          if result.state == RemovalState::Removed {
              conn.execute(
                  "DELETE FROM host_forgets WHERE id = ?1 AND state = 'pending' AND owner_id = ?2",
                  [id, &self.owner],
              )?;
              return Ok(());
          }
          // A final record keeps no roots (the review's item 12).
          conn.execute(
              "UPDATE host_forgets SET attempts = attempts + ?2, last_result = ?3, state = ?4,
                   agent_home = CASE WHEN ?4 = 'final' THEN NULL ELSE agent_home END
               WHERE id = ?1 AND state = 'pending' AND owner_id = ?5",
              params![
                  id,
                  i64::from(sent),
                  serde_json::to_string(result)?,
                  if done { "final" } else { "pending" },
                  self.owner
              ],
          )?;
          Ok(())
      }

      /// Whether another kept session of the record's host and agent now
      /// refers to its agent session (B8; re-checked before each attempt,
      /// the review's item 13).
      pub fn forget_is_shared(&self, record: &ForgetRecord) -> Result<bool> {
          let conn = self.conn();
          let in_use = agent_sessions_in_use(&conn, &self.owner, &record.host_id, &record.agent, &record.session_id)?;
          Ok(in_use.contains(&record.agent_session_id))
      }

      /// Dismiss a record (plan 9d O10): it is no longer retried or listed.
      /// `false` if there is none.
      pub fn dismiss_forget(&self, id: &str) -> Result<bool> {
          Ok(self.conn().execute(
              "DELETE FROM host_forgets WHERE id = ?1 AND owner_id = ?2",
              [id, &self.owner],
          )? > 0)
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  [&id, &self.owner],
              )?;
          }
          tx.commit()?;
          Ok(events)
      }
  ```

  with:

  ```rust
                  [&id, &self.owner],
              )?;
          }
          // A revoked host never connects again: what it was left to remove
          // stays listed, final (plan 9d O10).
          tx.execute(
              "UPDATE host_forgets SET state = 'final', last_result = ?2, agent_home = NULL
               WHERE host_id = ?1 AND state = 'pending' AND owner_id = ?3",
              params![
                  host_id,
                  serde_json::to_string(&final_result(ForgetReason::HostRevoked))?,
                  self.owner
              ],
          )?;
          tx.commit()?;
          Ok(events)
      }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  indexed,
                  ..
  ```

  with:

  ```rust
                  indexed,
                  agent_home,
                  ..
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
                  }
  ```

  with:

  ```rust
                      store_catalogue(&tx, &self.owner, session_id, indexed, &ts)?;
                      // Where the agent keeps this session's data (plan 9d
                      // decision 1, B8).
                      if let Some(home) = agent_home {
                          record_home(&tx, &self.owner, session_id, agent_session_id, home)?;
                      }
                  }
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                                  state.hub.resolve_session(&session_id, body.clone());
                              }
  ```

  with:

  ```rust
                                  state.hub.resolve_session(&session_id, body.clone());
                                  // A deleted session's adapter is gone now:
                                  // what an `attached` answer left goes again
                                  // (plan 9d O10). Only a tombstone has records.
                                  if matches!(body, SessionBody::SessionClosed) {
                                      crate::forget::retry_session(&state, &session_id);
                                  }
                              }
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                          }
                          for (_, held) in deferred.drain() {
  ```

  with:

  ```rust
                          }
                          // What deletes left this host to remove (plan 9d
                          // decision 5), now that requests may flow.
                          crate::forget::retry_host(&state, &host_id);
                          for (_, held) in deferred.drain() {
  ```

  In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
              frame @ (HostFrame::Projects { .. } | HostFrame::Directory { .. } | HostFrame::ResolvedPath { .. }) => {
  ```

  with:

  ```rust
              frame @ (HostFrame::Projects { .. }
              | HostFrame::Directory { .. }
              | HostFrame::ResolvedPath { .. }
              | HostFrame::SessionForgotten { .. }) => {
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
                  async move |_req: NewSessionRequest, responder, cx| match script.new_session_error {
                      Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                      None if script.config_in_update_only => {
                          if let Some(options) = announced() {
                              cx.send_notification(SessionNotification::new(
                                  "fake-session-1",
                                  SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                              ))?;
                          }
                          responder.respond(NewSessionResponse::new("fake-session-1"))
                      }
                      None => responder.respond(NewSessionResponse::new("fake-session-1").config_options(announced())),
  ```

  with:

  ```rust
                  async move |_req: NewSessionRequest, responder, cx| {
                      let id = script.session_id.clone().unwrap_or_else(|| "fake-session-1".into());
                      match script.new_session_error {
                          Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                          None if script.config_in_update_only => {
                              if let Some(options) = announced() {
                                  cx.send_notification(SessionNotification::new(
                                      id.clone(),
                                      SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                                  ))?;
                              }
                              responder.respond(NewSessionResponse::new(id))
                          }
                          None => responder.respond(NewSessionResponse::new(id).config_options(announced())),
                      }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
      pub prompt_updates: Vec<serde_json::Value>,
  }
  ```

  with:

  ```rust
      pub prompt_updates: Vec<serde_json::Value>,
      /// The agent session id `session/new` answers with, instead of
      /// `fake-session-1`: a lowercase UUID, as Claude's SDK makes (plan 9d).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub session_id: Option<String>,
  }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
              prompt_updates: Vec::new(),
          }
  ```

  with:

  ```rust
              prompt_updates: Vec::new(),
              session_id: None,
          }
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
    "$defs": {
      "AnswerRequest": {
  ```

  with:

  ```json
    "$defs": {
      "AgentHome": {
        "description": "Where an agent keeps its own data for a session, as its host resolved\nit from the adapter's environment (plan 9d decision 1): `root` is\nClaude's `CLAUDE_CONFIG_DIR` or `~/.claude`, Codex's `CODEX_HOME` or\n`~/.codex`; `sqlite_root` is Codex's `CODEX_SQLITE_HOME` if set.\nCanonical, its bytes as the filesystem gave them (O11). A host's report\nis not verified by the collector beyond its shape (`is_well_formed`).",
        "properties": {
          "root": {
            "type": "string"
          },
          "sqlite_root": {
            "type": [
              "string",
              "null"
            ]
          }
        },
        "required": [
          "root"
        ],
        "type": "object"
      },
      "AnswerRequest": {
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
            "description": "Resolving typed paths (`resolve_path`, kernel spec §5.4).",
            "type": "string"
  ```

  with:

  ```json
            "description": "Resolving typed paths (`resolve_path`, kernel spec §5.4).",
            "type": "string"
          },
          {
            "const": "forget_session",
            "description": "Removing a deleted session's transcript from the agent's own data\n(`forget_session`, plan 9d decision 4).",
            "type": "string"
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
              "hat_id"
            ],
            "type": "object"
          }
        ]
      },
  ```

  with:

  ```json
              "hat_id"
            ],
            "type": "object"
          },
          {
            "description": "Remove a deleted session's transcript from the agent's own data on\nthe host (plan 9d decision 4): only to a host with the\n`forget_session` capability. The host acts only on an exact match\nof its own registry (B1). Answered by `session_forgotten` |\n`error{invalid}`.",
            "properties": {
              "agent": {
                "type": "string"
              },
              "agent_home": {
                "$ref": "#/$defs/AgentHome"
              },
              "agent_session_id": {
                "type": "string"
              },
              "request_id": {
                "type": "string"
              },
              "type": {
                "const": "forget_session",
                "type": "string"
              }
            },
            "required": [
              "type",
              "request_id",
              "agent",
              "agent_session_id",
              "agent_home"
            ],
            "type": "object"
          }
        ]
      },
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
          "cred_kind"
        ],
  ```

  with:

  ```json
          "cred_kind"
        ],
        "type": "object"
      },
      "DeleteResult": {
        "description": "`DELETE /api/sessions/{id}` (ACP core §4.10; plan 9d decision 7).",
        "properties": {
          "host_transcript": {
            "$ref": "#/$defs/TranscriptRemoval"
          }
        },
        "required": [
          "host_transcript"
        ],
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
          "ts"
        ],
  ```

  with:

  ```json
          "ts"
        ],
        "type": "object"
      },
      "ForgetKind": {
        "description": "What a forget names on the host (plan 9d decision 4, B2): a kind of\nentry, never a path. The masked path each stands for is `masked`.",
        "oneOf": [
          {
            "enum": [
              "file_history",
              "session_env",
              "tasks",
              "debug"
            ],
            "type": "string"
          },
          {
            "const": "session",
            "description": "The whole forget, when it could not start (no home, an unknown id,\nan agent this host cannot forget for yet).",
            "type": "string"
          },
          {
            "const": "transcript",
            "description": "The transcript and its family in each project directory (B9).",
            "type": "string"
          },
          {
            "const": "codex_database_copies",
            "description": "Codex's own database copies of the conversation (plan 9d-ii).",
            "type": "string"
          }
        ]
      },
      "ForgetOutcome": {
        "description": "How a forget ended (plan 9d decision 4): `complete` when the check\nafterwards found nothing named left (B4).",
        "enum": [
          "complete",
          "partial"
        ],
        "type": "string"
      },
      "ForgetReason": {
        "description": "Why something named was not removed: a fixed code the host chooses\n(plan 9d B2), never free text.",
        "oneOf": [
          {
            "const": "attached",
            "description": "A live actor on the host has that agent session id (decision 13, B7).",
            "type": "string"
          },
          {
            "const": "in_progress",
            "description": "Another forget of the same agent session is running on the host\n(B7; the review's item 3).",
            "type": "string"
          },
          {
            "const": "unsupported_agent",
            "description": "This host cannot forget for that agent (yet).",
            "type": "string"
          },
          {
            "const": "no_recorded_home",
            "description": "The session recorded no agent home (decision 11).",
            "type": "string"
          },
          {
            "const": "unknown_to_host",
            "description": "The host's registry has no such (agent, id, home) (B1).",
            "type": "string"
          },
          {
            "const": "shared",
            "description": "Another kept session refers to the same agent session (B8).",
            "type": "string"
          },
          {
            "const": "unsafe_root",
            "description": "The root failed a check: `/`, `$HOME` or an ancestor, the host's data\ndirectory, not canonical, not the host user's, writable by others.",
            "type": "string"
          },
          {
            "const": "root_missing",
            "description": "The root is not there (any more).",
            "type": "string"
          },
          {
            "const": "symlink",
            "description": "A symlink, reported and never followed or removed (decision 8).",
            "type": "string"
          },
          {
            "const": "not_a_directory",
            "description": "A kind directory that is not a real directory.",
            "type": "string"
          },
          {
            "const": "unsafe_directory",
            "description": "A kind directory not the host user's, or writable by others (B3).",
            "type": "string"
          },
          {
            "const": "mount_point",
            "description": "The walk reached another file system (R2).",
            "type": "string"
          },
          {
            "const": "too_deep",
            "description": "The walk reached its depth bound (R2).",
            "type": "string"
          },
          {
            "const": "timed_out",
            "description": "The forget's deadline passed before the removal was done (B6).",
            "type": "string"
          },
          {
            "const": "still_present",
            "description": "Still there after the removal (B4).",
            "type": "string"
          },
          {
            "const": "io_error",
            "description": "The removal failed midway (B3).",
            "type": "string"
          },
          {
            "const": "invalid_id",
            "description": "The collector's own: the host answered `error{invalid}` for the id\n(decision 8, O10).",
            "type": "string"
          },
          {
            "const": "host_revoked",
            "description": "The collector's own: the host was revoked and never connects again\n(O10).",
            "type": "string"
          }
        ]
      },
      "ForgetRemaining": {
        "description": "Something a forget left (plan 9d decision 4). `retry: false` marks what\na retry cannot change.",
        "properties": {
          "reason": {
            "$ref": "#/$defs/ForgetReason"
          },
          "retry": {
            "type": "boolean"
          },
          "what": {
            "$ref": "#/$defs/ForgetWhat"
          }
        },
        "required": [
          "what",
          "reason",
          "retry"
        ],
        "type": "object"
      },
      "ForgetWhat": {
        "description": "One kind of entry and how many of them (plan 9d B2).",
        "properties": {
          "count": {
            "format": "uint32",
            "minimum": 0,
            "type": "integer"
          },
          "kind": {
            "$ref": "#/$defs/ForgetKind"
          }
        },
        "required": [
          "kind",
          "count"
        ],
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
          {
            "description": "The answer to `browse_directory` (ACP core §3.3, §7): the\nsubdirectories of `path`. A probe reply, like `projects`.",
  ```

  with:

  ```json
          {
            "description": "The answer to `forget_session` (plan 9d decision 4): what was\nremoved and what is left, as kinds and counts (B2). Like a probe's\nreply it is not outboxed and answers only the connection it was\nasked on: a forget is idempotent, so a lost answer costs a retry.",
            "properties": {
              "outcome": {
                "$ref": "#/$defs/ForgetOutcome"
              },
              "remaining": {
                "items": {
                  "$ref": "#/$defs/ForgetRemaining"
                },
                "type": "array"
              },
              "removed": {
                "items": {
                  "$ref": "#/$defs/ForgetWhat"
                },
                "type": "array"
              },
              "request_id": {
                "type": "string"
              },
              "type": {
                "const": "session_forgotten",
                "type": "string"
              }
            },
            "required": [
              "type",
              "request_id",
              "outcome",
              "removed",
              "remaining"
            ],
            "type": "object"
          },
          {
            "description": "The answer to `browse_directory` (ACP core §3.3, §7): the\nsubdirectories of `path`. A probe reply, like `projects`.",
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
        ],
        "type": "object"
      },
      "Indexed": {
  ```

  with:

  ```json
        ],
        "type": "object"
      },
      "HostRemovalItem": {
        "description": "One entry of `GET /api/settings/host-removals` (plan 9d decision 7).",
        "properties": {
          "agent": {
            "type": "string"
          },
          "attempts": {
            "format": "uint32",
            "minimum": 0,
            "type": "integer"
          },
          "created_at": {
            "type": "string"
          },
          "host_id": {
            "type": "string"
          },
          "id": {
            "type": "string"
          },
          "last_result": {
            "anyOf": [
              {
                "$ref": "#/$defs/TranscriptRemoval"
              },
              {
                "type": "null"
              }
            ]
          },
          "session_id": {
            "description": "The deleted session (a tombstone).",
            "type": "string"
          },
          "state": {
            "$ref": "#/$defs/HostRemovalState"
          }
        },
        "required": [
          "id",
          "host_id",
          "session_id",
          "agent",
          "state",
          "attempts",
          "created_at"
        ],
        "type": "object"
      },
      "HostRemovalState": {
        "description": "Whether a host removal is still retried (plan 9d decision 6, O10).",
        "oneOf": [
          {
            "const": "pending",
            "description": "Retried at the host's next handshake.",
            "type": "string"
          },
          {
            "const": "final",
            "description": "Done with something left that no retry changes: listed until it is\ndismissed.",
            "type": "string"
          }
        ]
      },
      "Indexed": {
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
      },
      "SessionBody": {
  ```

  with:

  ```json
      },
      "RemovalItem": {
        "description": "One kind of entry left on the host, how many, and why (plan 9d B2). Its\npath is `kind`'s masked one (`ForgetKind::masked`).",
        "properties": {
          "count": {
            "format": "uint32",
            "minimum": 0,
            "type": "integer"
          },
          "kind": {
            "$ref": "#/$defs/ForgetKind"
          },
          "reason": {
            "$ref": "#/$defs/ForgetReason"
          }
        },
        "required": [
          "kind",
          "count",
          "reason"
        ],
        "type": "object"
      },
      "RemovalPending": {
        "description": "Why a removal is still pending (plan 9d decision 7).",
        "oneOf": [
          {
            "enum": [
              "host_offline"
            ],
            "type": "string"
          },
          {
            "const": "host_needs_update",
            "description": "The host does not announce `forget_session`.",
            "type": "string"
          },
          {
            "const": "no_reply",
            "description": "No answer within the wait (30 s).",
            "type": "string"
          },
          {
            "const": "attached",
            "description": "The host still has the agent's session attached.",
            "type": "string"
          },
          {
            "const": "in_progress",
            "description": "Another attempt is running right now.",
            "type": "string"
          }
        ]
      },
      "RemovalState": {
        "description": "Where removing the agent's own transcript of a deleted session stands\n(plan 9d decision 7).",
        "oneOf": [
          {
            "const": "removed",
            "description": "Its host removed everything it names.",
            "type": "string"
          },
          {
            "const": "partial",
            "description": "Something is left (`remaining`), or could not be looked for.",
            "type": "string"
          },
          {
            "const": "pending",
            "description": "Not done yet (`pending`): retried when the host reconnects.",
            "type": "string"
          },
          {
            "const": "none",
            "description": "The session had no agent record: nothing on the host.",
            "type": "string"
          }
        ]
      },
      "SessionBody": {
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
            "properties": {
              "agent_session_id": {
  ```

  with:

  ```json
            "properties": {
              "agent_home": {
                "anyOf": [
                  {
                    "$ref": "#/$defs/AgentHome"
                  },
                  {
                    "type": "null"
                  }
                ],
                "description": "Where the agent keeps its data for this session (plan 9d\ndecision 1), registered on the host before this was sent (B1).\nAbsent from an older host, for an agent hennery cannot forget\nfor, or when the host could not resolve or register it."
              },
              "agent_session_id": {
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
      },
      "TurnOutcome": {
  ```

  with:

  ```json
      },
      "TranscriptRemoval": {
        "description": "The agent's own transcript of a deleted session on its host (plan 9d\ndecision 7): best effort. `notes` name what is never removed, whatever\nthe state.",
        "properties": {
          "notes": {
            "items": {
              "type": "string"
            },
            "type": "array"
          },
          "pending": {
            "anyOf": [
              {
                "$ref": "#/$defs/RemovalPending"
              },
              {
                "type": "null"
              }
            ]
          },
          "remaining": {
            "items": {
              "$ref": "#/$defs/RemovalItem"
            },
            "type": "array"
          },
          "state": {
            "$ref": "#/$defs/RemovalState"
          }
        },
        "required": [
          "state",
          "remaining",
          "notes"
        ],
        "type": "object"
      },
      "TurnOutcome": {
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  export type Capability = "projects" | "images" | "park" | "resolve_path";
  ```

  with:

  ```typescript
  export type Capability = "projects" | "images" | "park" | "resolve_path" | "forget_session";
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  export type SessionBody = { "kind": "session_started", request_id: string, agent_session_id: string, indexed: Indexed, } | { "kind": "start_failed", request_id: string, code: string, message: string, } | { "kind": "turn_started", request_id: string, turn_id: string, } | { "kind": "acp_update", indexed: Indexed, payload: unknown, } | { "kind": "turn_ended", turn_id: string, outcome: TurnOutcome, stop_reason?: string | null, error?: string | null, } | { "kind": "session_parked", reason: ParkReason, } | { "kind": "session_closed" } | { "kind": "adapter_exited", code?: number | null, signal?: number | null, stderr_tail: string, } | { "kind": "host_note", note: string, text: string, } | { "kind": "config_applied", request_id: string, indexed: Indexed, } | { "kind": "pending_opened", pending_id: string, indexed: Indexed, payload: unknown, } | { "kind": "pending_resolved", pending_id: string, resolution: PendingResolution, reason?: PendingReason | null, } | { "kind": "answer_result", pending_id: string, request_id: string, delivered: boolean, } | { "kind": "git_state", 
  ```

  with:

  ```typescript
  export type SessionBody = { "kind": "session_started", request_id: string, agent_session_id: string, indexed: Indexed, 
  /**
   * Where the agent keeps its data for this session (plan 9d
   * decision 1), registered on the host before this was sent (B1).
   * Absent from an older host, for an agent hennery cannot forget
   * for, or when the host could not resolve or register it.
   */
  agent_home?: AgentHome | undefined, } | { "kind": "start_failed", request_id: string, code: string, message: string, } | { "kind": "turn_started", request_id: string, turn_id: string, } | { "kind": "acp_update", indexed: Indexed, payload: unknown, } | { "kind": "turn_ended", turn_id: string, outcome: TurnOutcome, stop_reason?: string | null, error?: string | null, } | { "kind": "session_parked", reason: ParkReason, } | { "kind": "session_closed" } | { "kind": "adapter_exited", code?: number | null, signal?: number | null, stderr_tail: string, } | { "kind": "host_note", note: string, text: string, } | { "kind": "config_applied", request_id: string, indexed: Indexed, } | { "kind": "pending_opened", pending_id: string, indexed: Indexed, payload: unknown, } | { "kind": "pending_resolved", pending_id: string, resolution: PendingResolution, reason?: PendingReason | null, } | { "kind": "answer_result", pending_id: string, request_id: string, delivered: boolean, } | { "kind": "git_state", 
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  /**
   * Host -> collector.
  ```

  with:

  ```typescript
  /**
   * Where an agent keeps its own data for a session, as its host resolved
   * it from the adapter's environment (plan 9d decision 1): `root` is
   * Claude's `CLAUDE_CONFIG_DIR` or `~/.claude`, Codex's `CODEX_HOME` or
   * `~/.codex`; `sqlite_root` is Codex's `CODEX_SQLITE_HOME` if set.
   * Canonical, its bytes as the filesystem gave them (O11). A host's report
   * is not verified by the collector beyond its shape (`is_well_formed`).
   */
  export type AgentHome = { root: string, sqlite_root?: string | undefined, };

  /**
   * What a forget names on the host (plan 9d decision 4, B2): a kind of
   * entry, never a path. The masked path each stands for is `masked`.
   */
  export type ForgetKind = "session" | "transcript" | "file_history" | "session_env" | "tasks" | "debug" | "codex_database_copies";

  /**
   * Why something named was not removed: a fixed code the host chooses
   * (plan 9d B2), never free text.
   */
  export type ForgetReason = "attached" | "in_progress" | "unsupported_agent" | "no_recorded_home" | "unknown_to_host" | "shared" | "unsafe_root" | "root_missing" | "symlink" | "not_a_directory" | "unsafe_directory" | "mount_point" | "too_deep" | "timed_out" | "still_present" | "io_error" | "invalid_id" | "host_revoked";

  /**
   * One kind of entry and how many of them (plan 9d B2).
   */
  export type ForgetWhat = { kind: ForgetKind, count: number, };

  /**
   * Something a forget left (plan 9d decision 4). `retry: false` marks what
   * a retry cannot change.
   */
  export type ForgetRemaining = { what: ForgetWhat, reason: ForgetReason, retry: boolean, };

  /**
   * How a forget ended (plan 9d decision 4): `complete` when the check
   * afterwards found nothing named left (B4).
   */
  export type ForgetOutcome = "complete" | "partial";

  /**
   * Host -> collector.
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  home?: string | null, } | { "type": "directory", request_id: string, 
  ```

  with:

  ```typescript
  home?: string | null, } | { "type": "session_forgotten", request_id: string, outcome: ForgetOutcome, removed: Array<ForgetWhat>, remaining: Array<ForgetRemaining>, } | { "type": "directory", request_id: string, 
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  content: unknown[], } | { "type": "cancel_turn", request_id: string, session_id: string, turn_id: string, } | { "type": "set_config", request_id: string, session_id: string, config_id: string, value: ConfigValue, } | { "type": "answer_permission", request_id: string, session_id: string, pending_id: string, option_id: string, } | { "type": "answer_elicitation", request_id: string, session_id: string, pending_id: string, action: ElicitationAction, content?: unknown, } | { "type": "ack", session_id: string, ack_seq: number, } | { "type": "resolve_path", request_id: string, path: string, } | { "type": "park_session", request_id: string, session_id: string, } | { "type": "close_session", request_id: string, session_id: string, } | { "type": "list_projects", request_id: string, } | { "type": "browse_directory", request_id: string, path: string, } | { "type": "forget_hat", hat_id: string, };
  ```

  with:

  ```typescript
  content: unknown[], } | { "type": "cancel_turn", request_id: string, session_id: string, turn_id: string, } | { "type": "set_config", request_id: string, session_id: string, config_id: string, value: ConfigValue, } | { "type": "answer_permission", request_id: string, session_id: string, pending_id: string, option_id: string, } | { "type": "answer_elicitation", request_id: string, session_id: string, pending_id: string, action: ElicitationAction, content?: unknown, } | { "type": "ack", session_id: string, ack_seq: number, } | { "type": "resolve_path", request_id: string, path: string, } | { "type": "park_session", request_id: string, session_id: string, } | { "type": "close_session", request_id: string, session_id: string, } | { "type": "list_projects", request_id: string, } | { "type": "browse_directory", request_id: string, path: string, } | { "type": "forget_hat", hat_id: string, } | { "type": "forget_session", request_id: string, agent: string, agent_session_id: string, agent_home: AgentHome, };
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  token: string, };
  ```

  with:

  ```typescript
  token: string, };

  /**
   * Where removing the agent's own transcript of a deleted session stands
   * (plan 9d decision 7).
   */
  export type RemovalState = "removed" | "partial" | "pending" | "none";

  /**
   * Why a removal is still pending (plan 9d decision 7).
   */
  export type RemovalPending = "host_offline" | "host_needs_update" | "no_reply" | "attached" | "in_progress";

  /**
   * One kind of entry left on the host, how many, and why (plan 9d B2). Its
   * path is `kind`'s masked one (`ForgetKind::masked`).
   */
  export type RemovalItem = { kind: ForgetKind, count: number, reason: ForgetReason, };

  /**
   * The agent's own transcript of a deleted session on its host (plan 9d
   * decision 7): best effort. `notes` name what is never removed, whatever
   * the state.
   */
  export type TranscriptRemoval = { state: RemovalState, pending?: RemovalPending | undefined, remaining: Array<RemovalItem>, notes: Array<string>, };

  /**
   * `DELETE /api/sessions/{id}` (ACP core §4.10; plan 9d decision 7).
   */
  export type DeleteResult = { host_transcript: TranscriptRemoval, };

  /**
   * Whether a host removal is still retried (plan 9d decision 6, O10).
   */
  export type HostRemovalState = "pending" | "final";

  /**
   * One entry of `GET /api/settings/host-removals` (plan 9d decision 7).
   */
  export type HostRemovalItem = { id: string, host_id: string, 
  /**
   * The deleted session (a tombstone).
   */
  session_id: string, agent: string, state: HostRemovalState, attempts: number, last_result?: TranscriptRemoval | undefined, created_at: string, };
  ```


Run: `cargo run -p hennery-proto --bin gen`.

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-sessions -p hennery-testkit -p hennery-host --locked`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

Each listed in the lane's report and caught:
- the registry write, and its sync before `session_started`;
- the shape check of `agent_home`;
- the record's insert before the scrub;
- the scrub of `agent_home`;
- the `shared` rule, at the delete and before each send;
- each "done" and "final" branch;
- one attempt in flight, and the rerun;
- the step-up on the dismissal;
- the reply's caps;
- the capability gate;
- the 30 s wait.

- [ ] **Step 6: The full checks**

- [ ] **Step 7: Commit**

```bash
git add crates schema web
git -c commit.gpgsign=false commit -m "feat(sessions): record each session's agent home and send forget_session after a delete"
```

### Task 2: The purge forgets its sessions and counts them (9d-i-a)

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
  async fn parked_claude_session(collector: &Collector, host: &mut ScriptedHost) -> String {
      let c = client(collector);
  ```

  with:

  ```rust
  async fn parked_claude_session(collector: &Collector, host: &mut ScriptedHost) -> String {
      parked_claude_session_as(collector, host, AGENT_SESSION).await
  }

  /// `parked_claude_session`, its agent's session being `agent_session_id`.
  async fn parked_claude_session_as(collector: &Collector, host: &mut ScriptedHost, agent_session_id: &str) -> String {
      let c = client(collector);
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              agent_session_id: AGENT_SESSION.into(),
  ```

  with:

  ```rust
              agent_session_id: agent_session_id.into(),
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          (hennery_proto::rest::HostRemovalState::Final, 0)
      );
  }
  ```

  with:

  ```rust
          (hennery_proto::rest::HostRemovalState::Final, 0)
      );
  }

  /// Plan 9d decision 7, the 9c hand-off: a purge removes its sessions'
  /// transcripts on their hosts as a delete does: a record for each, written
  /// in each session's delete before its scrub, sent within the purge's one
  /// wait, and counted in `PurgeResult.host_transcripts`, with the sessions
  /// still pending.
  #[tokio::test]
  async fn a_purge_forgets_its_sessions_on_their_hosts_and_counts_them() {
      use hennery_proto::frames::{ForgetKind, ForgetReason, ForgetRemaining, ForgetWhat};
      const OTHER: &str = "1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let removed = parked_claude_session_as(&collector, &mut host, AGENT_SESSION).await;
      let waiting = parked_claude_session_as(&collector, &mut host, OTHER).await;
      // One from before 9d: an agent session, no home.
      let store = &collector.state.store;
      store
          .create_session("s-old", HOST, "claude", "/tmp", "x", None)
          .unwrap();
      store
          .ingest("s-old", 1, &SessionBody::session_started("r", "c0"))
          .unwrap();
      store.close_now("s-old").unwrap();
      let hat = match collector.state.hosts.create_hat("Acme", None, 0).unwrap() {
          hennery_kernel::hats::HatChange::Done(hat) => hat.id,
          other => panic!("{other:?}"),
      };
      for id in [&removed, &waiting, &"s-old".to_string()] {
          assert!(matches!(
              store.reassign_hat(id, &hat).unwrap(),
              hennery_sessions::store::Reassign::Done(_)
          ));
      }
      let c = client(&collector);
      let url = collector.url(&format!("/api/hats/{hat}/purge"));
      let call = tokio::spawn(async move {
          let resp = c.post(url).timeout(Duration::from_secs(40)).send().await.unwrap();
          (
              resp.status().as_u16(),
              resp.json::<Value>().await.unwrap_or(Value::Null),
          )
      });
      for _ in 0..2 {
          let CollectorFrame::ForgetSession {
              request_id,
              agent_session_id,
              ..
          } = host.next().await
          else {
              panic!("expected forget_session");
          };
          let left = if agent_session_id == OTHER {
              vec![ForgetRemaining {
                  what: ForgetWhat {
                      kind: ForgetKind::Session,
                      count: 0,
                  },
                  reason: ForgetReason::Attached,
                  retry: true,
              }]
          } else {
              vec![]
          };
          host.send(&forgotten(request_id, left)).await;
      }
      let (status, body) = call.await.unwrap();
      assert_eq!(status, 200, "{body}");
      assert_eq!(
          body["host_transcripts"],
          json!({ "removed": 1, "partial": 1, "pending": 1, "pending_sessions": [waiting] }),
          "{body}"
      );
      let listed: Vec<(String, hennery_proto::rest::HostRemovalState)> = removals(&collector)
          .await
          .into_iter()
          .map(|r| (r.session_id, r.state))
          .collect();
      assert_eq!(listed.len(), 2, "{listed:?}");
      assert!(listed.contains(&(waiting.clone(), hennery_proto::rest::HostRemovalState::Pending)));
      assert!(listed.contains(&("s-old".to_string(), hennery_proto::rest::HostRemovalState::Final)));
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test purge purge_forgets`
Expected: FAIL to compile: `no field host_transcripts on type PurgeResult`.

- [ ] **Step 3: The purge's forgets**

  In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::HostRemovalState,
          rest::HostRemovalItem,
      );
      out
  ```

  with:

  ```rust
          rest::HostRemovalState,
          rest::HostRemovalItem,
          rest::HostTranscripts,
      );
      out
  ```

  In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      pub unconfirmed: Vec<String>,
  }
  ```

  with:

  ```rust
      pub unconfirmed: Vec<String>,
      /// The agents' own transcripts of the purged sessions on their hosts
      /// (plan 9d decision 7).
      pub host_transcripts: HostTranscripts,
  }

  /// How many purged sessions' transcripts their hosts removed, removed in
  /// part, or have still to remove (plan 9d decision 7), as each session's
  /// `TranscriptRemoval.state`; a session with no agent record counts in
  /// none. The pending ones are retried at their host's next handshake.
  #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct HostTranscripts {
      #[ts(type = "number")]
      pub removed: u64,
      #[ts(type = "number")]
      pub partial: u64,
      #[ts(type = "number")]
      pub pending: u64,
      pub pending_sessions: Vec<String>,
  }
  ```

  In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      // is still pending. A purge's `PurgeResult` gets these as counts when
      // plan 9c rebases onto this (plan 9d decision 7): 9c hand-off.
  ```

  with:

  ```rust
      // is still pending. A purge counts these (`forget::after_purge`).
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
  use hennery_proto::rest::{HostRemovalItem, RemovalItem, RemovalPending, RemovalState, TranscriptRemoval};
  ```

  with:

  ```rust
  use hennery_proto::rest::{
      HostRemovalItem, HostTranscripts, RemovalItem, RemovalPending, RemovalState, TranscriptRemoval,
  };
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
      let deadline = tokio::time::Instant::now() + FORGET_WAIT;
  ```

  with:

  ```rust
      after_delete_until(state, forgets, tokio::time::Instant::now() + FORGET_WAIT).await
  }

  /// After a purge (plan 9d decision 7, O10): each purged session's records,
  /// as `after_delete` sends them, within one `FORGET_WAIT` for all of them,
  /// counted by each session's result. A session with no agent record counts
  /// in none.
  pub async fn after_purge(state: &AppState, purged: &[(String, HostForgets)]) -> HostTranscripts {
      let deadline = tokio::time::Instant::now() + FORGET_WAIT;
      let mut counts = HostTranscripts::default();
      for (session_id, forgets) in purged {
          match after_delete_until(state, forgets, deadline).await.state {
              RemovalState::Removed => counts.removed += 1,
              RemovalState::Partial => counts.partial += 1,
              RemovalState::Pending => {
                  counts.pending += 1;
                  counts.pending_sessions.push(session_id.clone());
              }
              RemovalState::None => {}
          }
      }
      counts
  }

  async fn after_delete_until(
      state: &AppState,
      forgets: &HostForgets,
      deadline: tokio::time::Instant,
  ) -> TranscriptRemoval {
  ```

  In `crates/hennery-sessions/src/hats.rs`, replace:

  ```rust
      pub unconfirmed: Vec<String>,
  }
  ```

  with:

  ```rust
      pub unconfirmed: Vec<String>,
      /// What each delete left for its host to remove (plan 9d decision 2),
      /// by session.
      pub forgets: Vec<(String, crate::store::HostForgets)>,
  }
  ```

  In `crates/hennery-sessions/src/hats.rs`, replace:

  ```rust
          Deletion::Done { event, unconfirmed, .. } => {
              state.hub.publish(event);
              purged.deleted += 1;
  ```

  with:

  ```rust
          Deletion::Done {
              event,
              unconfirmed,
              forgets,
          } => {
              state.hub.publish(event);
              purged.deleted += 1;
              purged.forgets.push((session.id.clone(), *forgets));
  ```

  In `crates/hennery-sessions/src/hats.rs`, replace:

  ```rust
      tracing::info!(hat_id = %id, sessions = purged.deleted, rules, "hat purged");
      Json(PurgeResult {
  ```

  with:

  ```rust
      tracing::info!(hat_id = %id, sessions = purged.deleted, rules, "hat purged");
      // The agents' own transcripts on the hosts, best effort, within one
      // wait for the whole purge (plan 9d decision 7, O10).
      let host_transcripts = crate::forget::after_purge(&state, &purged.forgets).await;
      Json(PurgeResult {
  ```

  In `crates/hennery-sessions/src/hats.rs`, replace:

  ```rust
          unconfirmed: purged.unconfirmed,
      })
  ```

  with:

  ```rust
          unconfirmed: purged.unconfirmed,
          host_transcripts,
      })
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
      },
      "Indexed": {
  ```

  with:

  ```json
      },
      "HostTranscripts": {
        "description": "How many purged sessions' transcripts their hosts removed, removed in\npart, or have still to remove (plan 9d decision 7), as each session's\n`TranscriptRemoval.state`; a session with no agent record counts in\nnone. The pending ones are retried at their host's next handshake.",
        "properties": {
          "partial": {
            "format": "uint64",
            "minimum": 0,
            "type": "integer"
          },
          "pending": {
            "format": "uint64",
            "minimum": 0,
            "type": "integer"
          },
          "pending_sessions": {
            "items": {
              "type": "string"
            },
            "type": "array"
          },
          "removed": {
            "format": "uint64",
            "minimum": 0,
            "type": "integer"
          }
        },
        "required": [
          "removed",
          "partial",
          "pending",
          "pending_sessions"
        ],
        "type": "object"
      },
      "Indexed": {
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
        "description": "200 to `POST /api/hats/{id}/purge` (plan 9c decision 10, A13): what\nthis purge deleted. A purge that resumes one that stopped counts only\nwhat it deleted itself.",
        "properties": {
          "rules": {
            "format": "uint64",
  ```

  with:

  ```json
        "description": "200 to `POST /api/hats/{id}/purge` (plan 9c decision 10, A13): what\nthis purge deleted. A purge that resumes one that stopped counts only\nwhat it deleted itself.",
        "properties": {
          "host_transcripts": {
            "$ref": "#/$defs/HostTranscripts",
            "description": "The agents' own transcripts of the purged sessions on their hosts\n(plan 9d decision 7)."
          },
          "rules": {
            "format": "uint64",
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
          "unconfirmed"
  ```

  with:

  ```json
          "unconfirmed",
          "host_transcripts"
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  unconfirmed: Array<string>, };
  ```

  with:

  ```typescript
  unconfirmed: Array<string>, 
  /**
   * The agents' own transcripts of the purged sessions on their hosts
   * (plan 9d decision 7).
   */
  host_transcripts: HostTranscripts, };
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  session_id: string, agent: string, state: HostRemovalState, attempts: number, last_result?: TranscriptRemoval | undefined, created_at: string, };
  ```

  with:

  ```typescript
  session_id: string, agent: string, state: HostRemovalState, attempts: number, last_result?: TranscriptRemoval | undefined, created_at: string, };

  /**
   * How many purged sessions' transcripts their hosts removed, removed in
   * part, or have still to remove (plan 9d decision 7), as each session's
   * `TranscriptRemoval.state`; a session with no agent record counts in
   * none. The pending ones are retried at their host's next handshake.
   */
  export type HostTranscripts = { removed: number, partial: number, pending: number, pending_sessions: Array<string>, };
  ```


- [ ] **Step 4: Run them to see them pass**

- [ ] **Step 5: Revert-probes**

These five lines were each probed and caught:
- the `after_purge` call;
- the collecting of the records;
- `delete_rows` writing them;
- the pending ids;
- the partial count.

- [ ] **Step 6: The full checks**

Expected: all pass; **1274 tests** on `e4e2ca3`.

- [ ] **Step 7: Commit**

```bash
git -c commit.gpgsign=false commit -am "feat(sessions): a purge's PurgeResult counts its sessions' host transcripts"
```

### Task 3: The Claude path on the host (9d-i-b)

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust

  /// The test's own data roots, canonical (`/var` is a link on macOS).
  ```

  with:

  ```rust

  /// The umask these tests assume: the root and kind directories must not be
  /// writable by group or others (B3), and a 002 umask would make every one
  /// so. Set for the whole test binary; every test here wants the same.
  fn usual_umask() {
      // SAFETY: umask(2) cannot fail.
      unsafe { libc::umask(0o022) };
  }

  /// The test's own data roots, canonical (`/var` is a link on macOS).
  ```

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust
      fn new() -> Self {
          let dir = tempfile::tempdir().unwrap();
  ```

  with:

  ```rust
      fn new() -> Self {
          usual_umask();
          let dir = tempfile::tempdir().unwrap();
  ```

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust
      assert_eq!(collector.client().delete(&url).send().await.unwrap().status(), 404);
  }
  ```

  with:

  ```rust
      assert_eq!(collector.client().delete(&url).send().await.unwrap().status(), 404);
  }

  /// The transcript and the other entries a Claude session leaves under its
  /// root, and one name beside them that is not the session's.
  fn claude_files(roots: &Roots) -> Vec<PathBuf> {
      let root = roots.claude();
      std::fs::create_dir_all(root.join("projects/-work")).unwrap();
      std::fs::create_dir_all(root.join(format!("file-history/{AGENT_SESSION}"))).unwrap();
      std::fs::create_dir_all(root.join("debug")).unwrap();
      let files = vec![
          root.join(format!("projects/-work/{AGENT_SESSION}.jsonl")),
          root.join(format!("projects/-work/{AGENT_SESSION}.ccr-tip.json")),
          root.join(format!("file-history/{AGENT_SESSION}/edit-1")),
          root.join(format!("debug/{AGENT_SESSION}.txt")),
      ];
      for file in &files {
          std::fs::write(file, "content").unwrap();
      }
      std::fs::write(root.join("projects/-work/other.jsonl"), "keep").unwrap();
      files
  }

  /// Decisions 7 and 8 end to end: a Claude session deleted over HTTP has
  /// its transcript removed on its host, `removed`, the context-clear note
  /// with it, and nothing names a path (B2).
  #[tokio::test]
  async fn a_delete_over_http_removes_the_claude_transcript_on_its_host() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      start_host(host_config(collector.addr, &roots, &answering(AGENT_SESSION)));
      connected(&collector, true).await;
      let session = start_session(&collector, "claude", &roots).await;
      let files = claude_files(&roots);
      let (result, body) = deleted(&collector, &session).await;
      assert_eq!(result.host_transcript.state, RemovalState::Removed, "{body}");
      assert!(result.host_transcript.notes[0].contains("context clear"), "{body}");
      for file in files {
          assert!(!file.exists(), "{}", file.display());
      }
      assert!(roots.claude().join("projects/-work/other.jsonl").exists());
      assert!(
          !body.contains(roots.base.to_str().unwrap()) && !body.contains("-work"),
          "{body}"
      );
      assert!(removals(&collector).await.is_empty());
  }

  /// Decision 5 end to end: deleted while its host is away, pending; at the
  /// host's return the transcript goes, and so does the record.
  #[tokio::test]
  async fn a_claude_transcript_deleted_while_its_host_was_away_goes_at_its_return() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      let cfg = host_config(collector.addr, &roots, &answering(AGENT_SESSION));
      let host = start_host(cfg.clone());
      connected(&collector, true).await;
      let session = start_session(&collector, "claude", &roots).await;
      host.abort();
      connected(&collector, false).await;
      let files = claude_files(&roots);
      let (result, _) = deleted(&collector, &session).await;
      assert_eq!(result.host_transcript.state, RemovalState::Pending);
      assert!(files.iter().all(|f| f.exists()));
      start_host(cfg);
      wait_for("the record removed at the host's return", || async {
          removals(&collector).await.is_empty().then_some(())
      })
      .await;
      for file in files {
          assert!(!file.exists(), "{}", file.display());
      }
  }
  ```

  Create `crates/hennery-testkit/tests/forget_claude.rs`:

  ```rust
  //! The Claude path of a forget on the host (plan 9d decision 8, B3–B6, B9,
  //! R1–R4), run directly against roots of the test's own: what it removes,
  //! what it leaves and never follows, and the roots it refuses. The fake
  //! adapter stands in for `claude-agent-acp`. These run on Linux CI as on
  //! macOS (R4).

  use hennery_host::AgentCommand;
  use hennery_host::forget::{Forget, ForgetContext, Forgotten, forget};
  use hennery_host::walk::Hooks;
  use hennery_proto::frames::{AgentHome, ForgetKind, ForgetOutcome, ForgetReason, ForgetWhat, HostFrame};
  use hennery_testkit::{FakeScript, SCRIPT_ENV};
  use std::collections::HashMap;
  use std::os::unix::fs::{PermissionsExt, symlink};
  use std::path::{Path, PathBuf};
  use std::sync::Arc;

  const ID: &str = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

  /// The transcript family in one project directory (B9).
  const FAMILY: [&str; 9] = [
      ".jsonl",
      "",
      ".ccr-tip.json",
      ".precompact.json",
      ".cast",
      ".dir-sync.json",
      ".dir-sync-empty.json",
      ".jsonl.superseded-1700000000",
      ".jsonl.compact.tmp.abc",
  ];

  /// Names beside them that are not the session's and stay.
  const NEIGHBOURS: [&str; 4] = [
      "1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3.jsonl",
      "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3x.jsonl",
      "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3.jsonl.bak",
      "notes.md",
  ];

  /// The umask these tests assume: the root and kind directories must not be
  /// writable by group or others (B3), and a 002 umask would make every one
  /// so. Set for the whole test binary; every test here wants the same.
  fn usual_umask() {
      // SAFETY: umask(2) cannot fail.
      unsafe { libc::umask(0o022) };
  }

  struct Root {
      _dir: tempfile::TempDir,
      base: PathBuf,
  }

  impl Root {
      fn new() -> Self {
          usual_umask();
          let dir = tempfile::tempdir().unwrap();
          let base = std::fs::canonicalize(dir.path()).unwrap();
          for sub in ["claude", "host", "home", "outside"] {
              std::fs::create_dir(base.join(sub)).unwrap();
          }
          Self { _dir: dir, base }
      }

      fn root(&self) -> PathBuf {
          self.base.join("claude")
      }

      fn at(&self, rel: &str) -> PathBuf {
          self.root().join(rel)
      }

      fn outside(&self) -> PathBuf {
          self.base.join("outside")
      }

      fn log(&self) -> PathBuf {
          self.base.join("delete.log")
      }

      fn ctx(&self, hooks: Hooks) -> ForgetContext {
          let script = FakeScript {
              delete_log: Some(self.log().to_str().unwrap().into()),
              ..FakeScript::default()
          };
          let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
          fake.env
              .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
          // The forget sets `CLAUDE_CONFIG_DIR` itself (decision 8); one in
          // the agent's own configuration, or one to strip, must not count.
          fake.env
              .push(("CLAUDE_CONFIG_DIR".into(), self.outside().to_str().unwrap().into()));
          fake.env
              .push(("CLAUDE_CODE_PROJECT_DIR_NAME".into(), "elsewhere".into()));
          ForgetContext {
              agents: HashMap::from([("claude".to_string(), fake)]),
              data_dir: self.base.join("host"),
              home: Some(self.base.join("home")),
              hooks,
          }
      }

      fn forget_at(&self, root: &Path) -> Forget {
          Forget {
              agent: "claude".into(),
              agent_session_id: ID.into(),
              agent_home: AgentHome {
                  root: root.to_str().unwrap().into(),
                  sqlite_root: None,
              },
          }
      }

      async fn forget(&self) -> Forgotten {
          forget(&self.ctx(Hooks::default()), &self.forget_at(&self.root())).await
      }

      /// The session's entries in every kind, and the neighbours that stay.
      fn populate(&self) {
          for project in ["projects/-p-one", "projects/-p-two"] {
              std::fs::create_dir_all(self.at(project)).unwrap();
              for suffix in FAMILY {
                  let path = self.at(&format!("{project}/{ID}{suffix}"));
                  if suffix.is_empty() {
                      std::fs::create_dir_all(path.join("subagents")).unwrap();
                      std::fs::write(path.join("subagents/agent-1.jsonl"), "x").unwrap();
                  } else {
                      std::fs::write(path, "transcript").unwrap();
                  }
              }
              for name in NEIGHBOURS {
                  std::fs::write(self.at(&format!("{project}/{name}")), "keep").unwrap();
              }
          }
          for kind in ["file-history", "session-env", "tasks"] {
              std::fs::create_dir_all(self.at(&format!("{kind}/{ID}/nested"))).unwrap();
              std::fs::write(self.at(&format!("{kind}/{ID}/nested/f")), "x").unwrap();
              std::fs::create_dir_all(self.at(&format!("{kind}/other"))).unwrap();
          }
          std::fs::create_dir_all(self.at("debug")).unwrap();
          std::fs::write(self.at(&format!("debug/{ID}.txt")), "x").unwrap();
          std::fs::write(self.at("debug/other.txt"), "keep").unwrap();
          std::fs::write(self.at("history.jsonl"), format!("{{\"sessionId\":\"{ID}\"}}")).unwrap();
      }

      /// Every path under the root, relative to it, sorted.
      fn tree(&self) -> Vec<String> {
          fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
              for entry in std::fs::read_dir(dir).unwrap() {
                  let path = entry.unwrap().path();
                  out.push(path.strip_prefix(base).unwrap().to_string_lossy().into_owned());
                  if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                      walk(base, &path, out);
                  }
              }
          }
          let mut out = Vec::new();
          walk(&self.root(), &self.root(), &mut out);
          out.sort();
          out
      }

      fn adapter_ran(&self) -> Option<String> {
          std::fs::read_to_string(self.log()).ok()
      }
  }

  fn reasons(forgotten: &Forgotten) -> Vec<(ForgetKind, ForgetReason, bool)> {
      forgotten
          .remaining
          .iter()
          .map(|r| (r.what.kind, r.reason, r.retry))
          .collect()
  }

  fn removed(forgotten: &Forgotten, kind: ForgetKind) -> u32 {
      forgotten
          .removed
          .iter()
          .filter(|w| w.kind == kind)
          .map(|w| w.count)
          .sum()
  }

  /// Decision 8, B9, B4, B6: the adapter's own delete runs, with
  /// `CLAUDE_CONFIG_DIR` the root, the root its cwd and the project-name
  /// override stripped; then exactly the session's names go, in every
  /// project directory and kind; nothing else does. A second forget finds
  /// nothing and is complete again (the adapter's not-found counts for
  /// nothing).
  #[tokio::test]
  async fn a_forget_removes_exactly_the_sessions_entries() {
      let root = Root::new();
      root.populate();
      let forgotten = root.forget().await;
      assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
      // The adapter took `<id>.jsonl` and `<id>/` of the first project
      // directory it found; the walk took the rest.
      assert_eq!(removed(&forgotten, ForgetKind::Transcript), 2 * FAMILY.len() as u32 - 2);
      for kind in [
          ForgetKind::FileHistory,
          ForgetKind::SessionEnv,
          ForgetKind::Tasks,
          ForgetKind::Debug,
      ] {
          assert_eq!(removed(&forgotten, kind), 1, "{kind:?}");
      }
      let mut kept = vec![
          "debug".to_string(),
          "debug/other.txt".into(),
          "history.jsonl".into(),
          "projects".into(),
      ];
      for project in ["projects/-p-one", "projects/-p-two"] {
          kept.push(project.into());
          kept.extend(NEIGHBOURS.iter().map(|n| format!("{project}/{n}")));
      }
      for kind in ["file-history", "session-env", "tasks"] {
          kept.push(kind.into());
          kept.push(format!("{kind}/other"));
      }
      kept.sort();
      assert_eq!(root.tree(), kept);
      let root_s = root.root().to_str().unwrap().to_string();
      assert_eq!(
          root.adapter_ran().unwrap(),
          format!("CLAUDE_CONFIG_DIR={root_s}\ncwd={root_s}\nCLAUDE_CODE_PROJECT_DIR_NAME=-\nCODEX_SQLITE_HOME=-\n")
      );
      // B2: what goes back names kinds and counts, never a path.
      let frame = serde_json::to_string(&forgotten.clone().into_frame("r".into())).unwrap();
      assert!(
          !frame.contains("-p-one") && !frame.contains(&root_s) && !frame.contains(ID),
          "{frame}"
      );
      let HostFrame::SessionForgotten { outcome, .. } = forgotten.into_frame("r".into()) else {
          unreachable!()
      };
      assert_eq!(outcome, ForgetOutcome::Complete);

      let again = root.forget().await;
      assert_eq!(
          (reasons(&again), again.removed.clone()),
          (vec![], Vec::<ForgetWhat>::new())
      );
  }

  /// R3, decision 8: a symlink at a named entry is reported and left, and
  /// what it points at is untouched.
  #[tokio::test]
  async fn a_symlink_at_a_named_entry_is_reported_and_never_followed() {
      let root = Root::new();
      std::fs::create_dir_all(root.outside().join("target")).unwrap();
      std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
      std::fs::create_dir_all(root.at("file-history")).unwrap();
      symlink(root.outside().join("target"), root.at(&format!("file-history/{ID}"))).unwrap();
      std::fs::create_dir_all(root.at("projects/-p")).unwrap();
      symlink(
          root.outside().join("target/precious"),
          root.at(&format!("projects/-p/{ID}.cast")),
      )
      .unwrap();
      let forgotten = root.forget().await;
      // What the links point at first: untouched (R3).
      assert_eq!(
          std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
          "keep"
      );
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Transcript, ForgetReason::Symlink, false),
              (ForgetKind::FileHistory, ForgetReason::Symlink, false),
          ]
      );
      assert!(std::fs::symlink_metadata(root.at(&format!("file-history/{ID}"))).is_ok());
      assert!(std::fs::symlink_metadata(root.at(&format!("projects/-p/{ID}.cast"))).is_ok());
  }

  /// R3, R1: a symlink inside a directory being removed is unlinked as an
  /// entry, never followed: what it points at is untouched.
  #[tokio::test]
  async fn a_symlink_inside_a_removed_directory_is_unlinked_not_followed() {
      let root = Root::new();
      std::fs::create_dir_all(root.outside().join("target")).unwrap();
      std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
      std::fs::create_dir_all(root.at(&format!("tasks/{ID}/deeper"))).unwrap();
      symlink(
          root.outside().join("target"),
          root.at(&format!("tasks/{ID}/deeper/link")),
      )
      .unwrap();
      let forgotten = root.forget().await;
      assert_eq!(
          std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
          "keep"
      );
      assert_eq!(reasons(&forgotten), []);
      assert!(std::fs::symlink_metadata(root.at(&format!("tasks/{ID}"))).is_err());
  }

  /// R3, R1: a directory swapped for a symlink after its parent was listed
  /// (the test hook) is not followed when the walk reaches it.
  #[tokio::test]
  async fn a_directory_swapped_for_a_symlink_after_listing_is_not_followed() {
      let root = Root::new();
      std::fs::create_dir_all(root.outside().join("target")).unwrap();
      std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
      std::fs::create_dir_all(root.at(&format!("session-env/{ID}/sub"))).unwrap();
      std::fs::write(root.at(&format!("session-env/{ID}/sub/f")), "x").unwrap();
      let listed = root.at(&format!("session-env/{ID}"));
      let (target, aside) = (root.outside().join("target"), root.outside().join("aside"));
      let swapped = Arc::new(std::sync::atomic::AtomicBool::new(false));
      let seen = swapped.clone();
      let hooks = Hooks {
          listed: Some(Arc::new(move |path: &Path, _entries: &[std::ffi::OsString]| {
              if path == listed && !seen.swap(true, std::sync::atomic::Ordering::SeqCst) {
                  std::fs::rename(path.join("sub"), &aside).unwrap();
                  symlink(&target, path.join("sub")).unwrap();
              }
          })),
      };
      let forgotten = forget(&root.ctx(hooks), &root.forget_at(&root.root())).await;
      assert!(swapped.load(std::sync::atomic::Ordering::SeqCst), "the hook never ran");
      assert_eq!(
          std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
          "keep"
      );
      assert_eq!(reasons(&forgotten), []);
      assert!(std::fs::symlink_metadata(root.at(&format!("session-env/{ID}"))).is_err());
  }

  /// B3: a kind directory that is a symlink is skipped and reported, and
  /// with `projects/` so, the adapter does not run (it would follow it).
  #[tokio::test]
  async fn a_symlinked_kind_directory_is_skipped_and_the_adapter_not_run() {
      let root = Root::new();
      std::fs::create_dir_all(root.outside().join("projects/-p")).unwrap();
      std::fs::write(root.outside().join(format!("projects/-p/{ID}.jsonl")), "keep").unwrap();
      symlink(root.outside().join("projects"), root.at("projects")).unwrap();
      std::fs::create_dir_all(root.at(&format!("tasks/{ID}"))).unwrap();
      let forgotten = root.forget().await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Transcript, ForgetReason::Symlink, false)]
      );
      assert_eq!(removed(&forgotten, ForgetKind::Tasks), 1);
      assert_eq!(root.adapter_ran(), None);
      assert_eq!(
          std::fs::read_to_string(root.outside().join(format!("projects/-p/{ID}.jsonl"))).unwrap(),
          "keep"
      );
  }

  /// B3: a kind directory writable by others, or one that is no directory,
  /// is skipped and reported.
  #[tokio::test]
  async fn a_kind_directory_writable_by_others_is_skipped() {
      let root = Root::new();
      std::fs::create_dir_all(root.at("debug")).unwrap();
      std::fs::write(root.at(&format!("debug/{ID}.txt")), "keep").unwrap();
      std::fs::set_permissions(root.at("debug"), std::fs::Permissions::from_mode(0o777)).unwrap();
      std::fs::write(root.at("tasks"), "not a directory").unwrap();
      let forgotten = root.forget().await;
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Tasks, ForgetReason::NotADirectory, false),
              (ForgetKind::Debug, ForgetReason::UnsafeDirectory, false),
          ]
      );
      assert!(root.at(&format!("debug/{ID}.txt")).exists());
  }

  /// Decision 8, B3: the root must be canonical, a real directory of the
  /// host user's, not writable by others, and none of `/`, the host user's
  /// home or one of its ancestors, the host's data directory or one of its
  /// ancestors. Refused, nothing runs and nothing goes.
  #[tokio::test]
  async fn unsafe_roots_are_refused_before_anything_runs() {
      let root = Root::new();
      std::fs::create_dir_all(root.base.join("home/projects/-p")).unwrap();
      std::fs::write(root.base.join(format!("home/projects/-p/{ID}.jsonl")), "keep").unwrap();
      symlink(root.root(), root.base.join("linked")).unwrap();
      let cases: Vec<(PathBuf, ForgetReason)> = vec![
          (PathBuf::from("/"), ForgetReason::UnsafeRoot),
          (root.base.join("home"), ForgetReason::UnsafeRoot),
          (root.base.clone(), ForgetReason::UnsafeRoot),
          (root.base.join("host"), ForgetReason::UnsafeRoot),
          (root.base.join("linked"), ForgetReason::UnsafeRoot),
          (root.base.join("claude/../claude"), ForgetReason::UnsafeRoot),
          (root.base.join("gone"), ForgetReason::RootMissing),
      ];
      let ctx = root.ctx(Hooks::default());
      for (path, reason) in cases {
          let forgotten = forget(&ctx, &root.forget_at(&path)).await;
          assert_eq!(
              reasons(&forgotten),
              [(ForgetKind::Session, reason, false)],
              "{}",
              path.display()
          );
      }
      std::fs::set_permissions(root.root(), std::fs::Permissions::from_mode(0o775)).unwrap();
      let forgotten = root.forget().await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Session, ForgetReason::UnsafeRoot, false)]
      );
      assert_eq!(root.adapter_ran(), None);
      assert!(root.base.join(format!("home/projects/-p/{ID}.jsonl")).exists());
  }

  /// R2: a tree deeper than the walk's bound stops there, reported.
  #[tokio::test]
  async fn a_tree_past_the_depth_bound_is_left_reported() {
      let root = Root::new();
      let mut deep = root.at(&format!("tasks/{ID}"));
      for _ in 0..hennery_host::walk::MAX_DEPTH + 2 {
          deep = deep.join("d");
      }
      std::fs::create_dir_all(&deep).unwrap();
      let forgotten = root.forget().await;
      assert_eq!(reasons(&forgotten), [(ForgetKind::Tasks, ForgetReason::TooDeep, false)]);
  }

  /// Decision 8: other agents are not forgotten yet (9d-ii).
  #[tokio::test]
  async fn a_codex_forget_is_unsupported_for_now() {
      let root = Root::new();
      let mut codex = root.forget_at(&root.root());
      codex.agent = "codex".into();
      let forgotten = forget(&root.ctx(Hooks::default()), &codex).await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Session, ForgetReason::UnsupportedAgent, true)]
      );
  }

  /// Decision 8: a project directory that is a symlink is skipped by the
  /// walk, never followed. (Here with no adapter configured: the adapter's
  /// own delete is outside the no-follow guarantee, B3.)
  #[tokio::test]
  async fn a_symlinked_project_directory_is_not_walked() {
      let root = Root::new();
      std::fs::create_dir_all(root.outside().join("proj")).unwrap();
      std::fs::write(root.outside().join(format!("proj/{ID}.jsonl")), "keep").unwrap();
      std::fs::create_dir_all(root.at("projects")).unwrap();
      symlink(root.outside().join("proj"), root.at("projects/-linked")).unwrap();
      let mut ctx = root.ctx(Hooks::default());
      ctx.agents.clear();
      let forgotten = forget(&ctx, &root.forget_at(&root.root())).await;
      assert_eq!(reasons(&forgotten), []);
      assert_eq!(
          std::fs::read_to_string(root.outside().join(format!("proj/{ID}.jsonl"))).unwrap(),
          "keep"
      );
  }

  /// B4, B3: an entry the removal could not finish is found by the check
  /// afterwards and reported, retryable (here a directory the host user may
  /// not write; the check is skipped for a root user, who may).
  #[tokio::test]
  async fn an_entry_the_removal_could_not_finish_is_reported_for_a_retry() {
      // SAFETY: geteuid(2) cannot fail.
      if unsafe { libc::geteuid() } == 0 {
          return;
      }
      let root = Root::new();
      let locked = root.at(&format!("tasks/{ID}/locked"));
      std::fs::create_dir_all(&locked).unwrap();
      std::fs::write(locked.join("f"), "x").unwrap();
      std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
      let forgotten = root.forget().await;
      std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
      assert_eq!(reasons(&forgotten), [(ForgetKind::Tasks, ForgetReason::IoError, true)]);
      assert!(locked.join("f").exists());
  }
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust

  struct Setup {
  ```

  with:

  ```rust

  /// The umask these tests assume: the root and kind directories must not be
  /// writable by group or others (B3), and a 002 umask would make every one
  /// so. Set for the whole test binary; every test here wants the same.
  fn usual_umask() {
      // SAFETY: umask(2) cannot fail.
      unsafe { libc::umask(0o022) };
  }

  struct Setup {
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
      async fn new() -> Self {
          let dir = tempfile::tempdir().unwrap();
  ```

  with:

  ```rust
      async fn new() -> Self {
          usual_umask();
          let dir = tempfile::tempdir().unwrap();
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
          let script = FakeScript {
              session_id: Some(AGENT_SESSION.into()),
              ..FakeScript::default()
          };
  ```

  with:

  ```rust
          self.start_host_with(FakeScript {
              session_id: Some(AGENT_SESSION.into()),
              ..FakeScript::default()
          });
      }

      fn start_host_with(&self, script: FakeScript) {
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
      assert_eq!(*outcome, ForgetOutcome::Partial);
      assert_eq!(
          reasons(&ran),
          [(ForgetKind::Session, ForgetReason::UnsupportedAgent, true)]
      );
  ```

  with:

  ```rust
      // Nothing of the session is under the root: complete.
      assert_eq!(*outcome, ForgetOutcome::Complete);
      assert_eq!(reasons(&ran), []);
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
      );
  }
  ```

  with:

  ```rust
      );
  }

  /// B7: while a forget runs, the agent session it removes is not attached:
  /// a resume of it is refused `forgetting`, and attaches once it is over.
  #[tokio::test]
  async fn an_attach_waits_out_a_forget_of_the_same_agent_session() {
      let setup = Setup::new().await;
      let (log, gate) = (setup.base.join("delete.log"), setup.base.join("gate"));
      setup.start_host_with(FakeScript {
          session_id: Some(AGENT_SESSION.into()),
          delete_log: Some(log.to_str().unwrap().into()),
          delete_waits_for_file: Some(gate.to_str().unwrap().into()),
          ..FakeScript::default()
      });
      let (mut collector, _) = Collector::accept(&setup.listener).await;
      collector.send(&setup.start()).await;
      let SessionBody::SessionStarted { .. } = collector.fact().await else {
          panic!("expected session_started");
      };
      collector
          .send(&CollectorFrame::CloseSession {
              request_id: "c1".into(),
              session_id: "s1".into(),
          })
          .await;
      loop {
          if let SessionBody::SessionClosed = collector.fact().await {
              break;
          }
      }
      collector
          .send(&CollectorFrame::ForgetSession {
              request_id: "f1".into(),
              agent: "claude".into(),
              agent_session_id: AGENT_SESSION.into(),
              agent_home: home(&setup.root()),
          })
          .await;
      // The forget's adapter has its `session/delete`, and holds it.
      let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
      while !log.exists() {
          assert!(
              tokio::time::Instant::now() < deadline,
              "the forget's adapter never got session/delete"
          );
          tokio::time::sleep(Duration::from_millis(20)).await;
      }
      // A second forget of the same agent session meanwhile: refused, in
      // progress, retryable (the review's item 3).
      let second = collector.forget("f2", AGENT_SESSION, &setup.root()).await.unwrap();
      assert_eq!(
          reasons(&second),
          [(ForgetKind::Session, ForgetReason::InProgress, true)]
      );
      let resume = |request_id: &str| CollectorFrame::ResumeSession {
          request_id: request_id.into(),
          session_id: "s1".into(),
          committed_seq: 0,
          agent: "claude".into(),
          cwd: setup.base.join("work").to_str().unwrap().into(),
          agent_session_id: AGENT_SESSION.into(),
          config: Default::default(),
      };
      collector.send(&resume("r2")).await;
      loop {
          match collector.next().await {
              HostFrame::Error { request_id, code, .. } if request_id == "r2" => {
                  assert_eq!(code, "forgetting");
                  break;
              }
              HostFrame::Session { body, .. } => panic!("attached during the forget: {body:?}"),
              _ => {}
          }
      }
      std::fs::write(&gate, "").unwrap();
      loop {
          if let frame @ HostFrame::SessionForgotten { .. } = collector.next().await {
              assert_eq!(frame.probe_request_id(), Some("f1"));
              break;
          }
      }
      collector.send(&resume("r3")).await;
      let SessionBody::SessionStarted { request_id, .. } = collector.fact().await else {
          panic!("expected session_started");
      };
      assert_eq!(request_id, "r3");
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test forget_claude --test forget_host`
Expected: FAIL. A Claude forget is still answered `unsupported_agent`.

- [ ] **Step 3: The descriptor walk and the Claude forget**

  In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
      pub fn spawn(agent: &AgentCommand, cwd: &Path) -> std::io::Result<(Self, AdapterIo)> {
          // Read before the fork: getrlimit is not async-signal-safe.
  ```

  with:

  ```rust
      pub fn spawn(agent: &AgentCommand, cwd: &Path) -> std::io::Result<(Self, AdapterIo)> {
          Self::spawn_stripped(agent, cwd, &[])
      }

      /// Like `spawn`, with `strip` removed from the adapter's environment
      /// too, whoever set them (plan 9d B6: a forget's adapter).
      pub fn spawn_stripped(agent: &AgentCommand, cwd: &Path, strip: &[&str]) -> std::io::Result<(Self, AdapterIo)> {
          // Read before the fork: getrlimit is not async-signal-safe.
  ```

  In `crates/hennery-host/src/adapter.rs`, replace:

  ```rust
          for var in NESTING_VARS.iter().chain(HOST_SECRET_VARS).chain(HOST_LOG_VARS) {
  ```

  with:

  ```rust
          for var in NESTING_VARS
              .iter()
              .chain(HOST_SECRET_VARS)
              .chain(HOST_LOG_VARS)
              .chain(strip)
          {
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          home: cfg.home.clone(),
      };
  ```

  with:

  ```rust
          home: cfg.home.clone(),
          hooks: crate::walk::Hooks::default(),
      };
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  use crate::adapter::AgentCommand;
  use hennery_proto::frames::{ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
  use std::collections::HashMap;
  use std::path::PathBuf;
  use std::time::Duration;
  ```

  with:

  ```rust
  use crate::adapter::{Adapter, AgentCommand};
  use crate::walk;
  use agent_client_protocol::schema::ProtocolVersion;
  use agent_client_protocol::schema::v1::{DeleteSessionRequest, InitializeRequest, SessionId};
  use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder, UntypedMessage};
  use hennery_proto::frames::{ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
  use std::collections::{BTreeMap, HashMap};
  use std::os::fd::{AsRawFd, OwnedFd, RawFd};
  use std::os::unix::ffi::OsStrExt;
  use std::path::{Path, PathBuf};
  use std::time::Duration;
  use tokio::time::Instant;
  use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      pub home: Option<PathBuf>,
  }
  ```

  with:

  ```rust
      pub home: Option<PathBuf>,
      /// Test seams of the removal (the `test-hooks` feature): none in a
      /// real build.
      pub hooks: crate::walk::Hooks,
  }
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  /// Run one forget (plan 9d decision 8). Only Claude's data is removed so
  /// far; any other agent is answered `unsupported_agent`, retryable, for
  /// plan 9d-ii to take up.
  pub async fn forget(_ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      let _ = &forget.agent_home;
      Forgotten {
          removed: Vec::new(),
          remaining: vec![left(ForgetKind::Session, 0, ForgetReason::UnsupportedAgent, true)],
      }
  ```

  with:

  ```rust
  /// Run one forget (plan 9d decision 8) within `FORGET_DEADLINE`. Only
  /// Claude's data is removed so far; any other agent is answered
  /// `unsupported_agent`, retryable, for plan 9d-ii to take up.
  pub async fn forget(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      if forget.agent != crate::agent_home::CLAUDE {
          return Forgotten {
              removed: Vec::new(),
              remaining: vec![left(ForgetKind::Session, 0, ForgetReason::UnsupportedAgent, true)],
          };
      }
      if !valid_id(&forget.agent_session_id) {
          // The connection checked it; nothing is built from one that is not.
          return Forgotten {
              removed: Vec::new(),
              remaining: vec![left(ForgetKind::Session, 0, ForgetReason::InvalidId, false)],
          };
      }
      forget_claude(ctx, forget).await
  }

  /// Claude (decision 8, B3–B6, B9): check the root, then its kind
  /// directories through descriptors, then run the adapter's own delete
  /// (unless `projects/` failed its check: the adapter would follow it), then
  /// remove the exact names through the descriptor walk, then check what is
  /// left (B4).
  async fn forget_claude(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      let until = Instant::now() + FORGET_DEADLINE;
      let root = PathBuf::from(&forget.agent_home.root);
      let checked = {
          let (ctx, root) = (ctx.clone(), root.clone());
          tokio::task::spawn_blocking(move || check(&ctx, &root)).await
      };
      let kinds = match checked {
          Ok(Ok(kinds)) => kinds,
          Ok(Err(reason)) => {
              return Forgotten {
                  removed: Vec::new(),
                  remaining: vec![left(ForgetKind::Session, 0, reason, retryable(reason))],
              };
          }
          Err(_) => {
              return Forgotten {
                  removed: Vec::new(),
                  remaining: vec![left(ForgetKind::Session, 0, ForgetReason::IoError, true)],
              };
          }
      };
      if kinds.dirs[0].2.is_ok() {
          run_adapter(ctx, forget, &root, until).await;
      }
      let (id, hooks) = (forget.agent_session_id.clone(), ctx.hooks.clone());
      match tokio::task::spawn_blocking(move || remove_and_verify(&kinds, &root, &id, &hooks)).await {
          Ok(forgotten) => forgotten,
          Err(_) => Forgotten {
              removed: Vec::new(),
              remaining: vec![left(ForgetKind::Session, 0, ForgetReason::IoError, true)],
          },
      }
  }

  /// Variables a forget's adapter never inherits unless recorded (B6):
  /// another project directory name would aim Claude's delete elsewhere, and
  /// another SQLite home Codex's.
  pub const FORGET_STRIPPED_VARS: &[&str] = &["CLAUDE_CODE_PROJECT_DIR_NAME", "CODEX_SQLITE_HOME"];

  /// How long the adapter gets of the forget's deadline (B6); the rest is
  /// for stopping it and for the removal.
  const ADAPTER_SHARE: Duration = Duration::from_secs(12);

  /// The grace a forget's adapter gets between SIGTERM and SIGKILL.
  const ADAPTER_GRACE: Duration = Duration::from_secs(2);

  /// The agent's own delete (decision 8): its adapter, through
  /// `Adapter::spawn`'s hygiene (B6), with `CLAUDE_CONFIG_DIR` set to the
  /// root and the root as its cwd, then `initialize`, then `session/delete`
  /// if it advertises it. Whatever it answers (not found included) counts
  /// for nothing: the check afterwards decides (B4). Outside the no-follow
  /// guarantee: the adapter resolves its own paths. Its group is killed
  /// after, within the deadline.
  async fn run_adapter(ctx: &ForgetContext, forget: &Forget, root: &Path, until: Instant) {
      let Some(mut command) = ctx.agents.get(&forget.agent).cloned() else {
          tracing::info!(agent = %forget.agent, "no adapter configured for a forget; only the exact entries go");
          return;
      };
      command
          .env
          .push(("CLAUDE_CONFIG_DIR".into(), root.to_string_lossy().into_owned()));
      let (mut adapter, io) = match Adapter::spawn_stripped(&command, root, FORGET_STRIPPED_VARS) {
          Ok(spawned) => spawned,
          Err(err) => {
              tracing::warn!(agent = %forget.agent, error = %err, "a forget's adapter did not spawn");
              return;
          }
      };
      let transport = ByteStreams::new(io.stdin.compat_write(), io.stdout.compat());
      let id = forget.agent_session_id.clone();
      let talk = Client
          .builder()
          .name("hennery-host")
          .on_receive_notification(
              async move |_msg: UntypedMessage, _cx| Ok(()),
              agent_client_protocol::on_receive_notification!(),
          )
          .on_receive_request(
              async move |msg: UntypedMessage, responder: Responder<serde_json::Value>, _cx| {
                  responder.respond_with_error(agent_client_protocol::Error::method_not_found().data(msg.method))
              },
              agent_client_protocol::on_receive_request!(),
          )
          .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
              let init = conn
                  .send_request(InitializeRequest::new(ProtocolVersion::V1))
                  .block_task()
                  .await?;
              if init.agent_capabilities.session_capabilities.delete.is_some() {
                  // Not found is success (the SDK throws for it); any answer
                  // is only logged.
                  if let Err(err) = conn
                      .send_request(DeleteSessionRequest::new(SessionId::new(id)))
                      .block_task()
                      .await
                  {
                      tracing::info!(error = %err, "the adapter's session/delete answered an error");
                  }
              }
              Ok(())
          });
      let deadline = until.min(Instant::now() + ADAPTER_SHARE);
      match tokio::time::timeout_at(deadline, talk).await {
          Ok(Ok(())) => {}
          Ok(Err(err)) => tracing::info!(error = %err, "a forget's adapter connection ended"),
          Err(_) => tracing::warn!("a forget's adapter ran out of time"),
      }
      adapter.terminate(ADAPTER_GRACE).await;
  }

  /// The kind directories under a Claude root, each with the kind it holds
  /// (decision 8).
  const CLAUDE_KINDS: [(ForgetKind, &str); 5] = [
      (ForgetKind::Transcript, "projects"),
      (ForgetKind::FileHistory, "file-history"),
      (ForgetKind::SessionEnv, "session-env"),
      (ForgetKind::Tasks, "tasks"),
      (ForgetKind::Debug, "debug"),
  ];

  /// Whether `name` is one of the transcript's own names in a project
  /// directory (B9): exactly `<id>.jsonl`, `<id>`, `<id>.ccr-tip.json`,
  /// `<id>.precompact.json`, `<id>.cast`, `<id>.dir-sync.json`,
  /// `<id>.dir-sync-empty.json`, or one of the prefixes
  /// `<id>.jsonl.superseded-` and `<id>.jsonl.compact.tmp.`. The id is a
  /// checked one (`valid_id`); nothing else is ever matched.
  pub fn in_transcript_family(name: &[u8], id: &str) -> bool {
      let Some(rest) = name.strip_prefix(id.as_bytes()) else {
          return false;
      };
      matches!(
          rest,
          b"" | b".jsonl"
              | b".ccr-tip.json"
              | b".precompact.json"
              | b".cast"
              | b".dir-sync.json"
              | b".dir-sync-empty.json"
      ) || rest.starts_with(b".jsonl.superseded-")
          || rest.starts_with(b".jsonl.compact.tmp.")
  }

  /// The one entry of a non-transcript kind (decision 8): `<id>` in
  /// `file-history`, `session-env` and `tasks`, `<id>.txt` in `debug`.
  fn kind_entry(kind: ForgetKind, id: &str) -> String {
      match kind {
          ForgetKind::Debug => format!("{id}.txt"),
          _ => id.to_string(),
      }
  }

  /// The host user's own, and not writable by group or others (B3).
  fn safely_owned(st: &libc::stat) -> bool {
      // SAFETY: geteuid(2) cannot fail.
      st.st_uid == unsafe { libc::geteuid() } && st.st_mode & 0o022 == 0
  }

  /// The root, checked (decision 8, B3), open: absolute and canonical, still
  /// resolving to itself; not `/`, the host user's home or an ancestor of
  /// it, nor the host's data directory or an ancestor of it; a real
  /// directory of the host user's, not writable by others. With its device.
  fn open_checked_root(ctx: &ForgetContext, root: &Path) -> Result<(OwnedFd, libc::dev_t), ForgetReason> {
      if !root.is_absolute() {
          return Err(ForgetReason::UnsafeRoot);
      }
      match std::fs::canonicalize(root) {
          Ok(canonical) if canonical == root => {}
          Ok(_) => return Err(ForgetReason::UnsafeRoot),
          Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(ForgetReason::RootMissing),
          Err(_) => return Err(ForgetReason::UnsafeRoot),
      }
      let mut guarded: Vec<PathBuf> = vec![ctx.data_dir.clone()];
      guarded.extend(ctx.home.clone());
      guarded.extend(std::env::var_os("HOME").map(PathBuf::from));
      for guarded in guarded {
          let guarded = std::fs::canonicalize(&guarded).unwrap_or(guarded);
          // `root` is that directory, or one of its ancestors (`/` too).
          if guarded.starts_with(root) {
              return Err(ForgetReason::UnsafeRoot);
          }
      }
      let fd = walk::open_root(root).map_err(|_| ForgetReason::UnsafeRoot)?;
      let st = walk::stat_fd(fd.as_raw_fd()).map_err(|_| ForgetReason::UnsafeRoot)?;
      if !walk::is_dir(&st) || !safely_owned(&st) {
          return Err(ForgetReason::UnsafeRoot);
      }
      Ok((fd, st.st_dev))
  }

  /// A kind directory, checked before anything runs (B3): open through no
  /// symlink, the host user's, not writable by others, on the root's file
  /// system. `Ok(None)` if there is none.
  fn open_kind(root: RawFd, name: &str, dev: libc::dev_t) -> Result<Option<OwnedFd>, ForgetReason> {
      let c_name = walk::c_name(name.as_bytes());
      let fd = match walk::open_dir_at(root, &c_name) {
          Ok(fd) => fd,
          Err(libc::ENOENT) => return Ok(None),
          // A symlink is refused as `ELOOP` on some systems and `ENOTDIR`
          // on others (macOS): which it is, is read without following it.
          Err(libc::ELOOP | libc::ENOTDIR) => {
              return Err(match walk::stat_at(root, &c_name) {
                  Ok(Some(st)) if walk::is_link(&st) => ForgetReason::Symlink,
                  _ => ForgetReason::NotADirectory,
              });
          }
          Err(_) => return Err(ForgetReason::IoError),
      };
      let st = walk::stat_fd(fd.as_raw_fd()).map_err(|_| ForgetReason::IoError)?;
      if st.st_dev != dev {
          return Err(ForgetReason::MountPoint);
      }
      if !safely_owned(&st) {
          return Err(ForgetReason::UnsafeDirectory);
      }
      Ok(Some(fd))
  }

  /// A retry can change what is left for `reason` (decision 4): an entry
  /// still there, or a removal that failed midway (B3). A symlink, a mount
  /// point, an unsafe directory stay as they are.
  fn retryable(reason: ForgetReason) -> bool {
      matches!(reason, ForgetReason::StillPresent | ForgetReason::IoError)
  }

  /// What is counted per kind.
  #[derive(Default)]
  struct Tally {
      removed: BTreeMap<ForgetKind, u32>,
      left: BTreeMap<(ForgetKind, ForgetReason), u32>,
  }

  impl Tally {
      fn left(&mut self, kind: ForgetKind, reason: ForgetReason) {
          *self.left.entry((kind, reason)).or_default() += 1;
      }

      fn into_forgotten(self) -> Forgotten {
          Forgotten {
              removed: self
                  .removed
                  .into_iter()
                  .map(|(kind, count)| ForgetWhat { kind, count })
                  .collect(),
              remaining: self
                  .left
                  .into_iter()
                  .map(|((kind, reason), count)| left(kind, count, reason, retryable(reason)))
                  .collect(),
          }
      }
  }

  fn stop_reason(stop: walk::Stop) -> ForgetReason {
      match stop {
          walk::Stop::MountPoint => ForgetReason::MountPoint,
          walk::Stop::TooDeep => ForgetReason::TooDeep,
          walk::Stop::Io(_) => ForgetReason::IoError,
      }
  }

  /// One kind directory as checked: its kind, its name, and it open (none
  /// there), or why it is skipped.
  type KindDir = (ForgetKind, &'static str, Result<Option<OwnedFd>, ForgetReason>);

  /// The checked root's kind directories, as opened before the adapter ran.
  struct Kinds {
      root: OwnedFd,
      dev: libc::dev_t,
      dirs: Vec<KindDir>,
  }

  /// The root and its kind directories, checked (B3). `Err` for the root.
  fn check(ctx: &ForgetContext, root: &Path) -> Result<Kinds, ForgetReason> {
      let (root_fd, dev) = open_checked_root(ctx, root)?;
      let dirs = CLAUDE_KINDS
          .iter()
          .map(|&(kind, name)| (kind, name, open_kind(root_fd.as_raw_fd(), name, dev)))
          .collect();
      Ok(Kinds {
          root: root_fd,
          dev,
          dirs,
      })
  }

  /// The project directories under `projects` that are real directories on
  /// `dev`, each open, with its name; a symlinked one is skipped, never
  /// followed (decision 8); one on another file system is counted.
  /// `base` is the directory's path, for the logs only: nothing is looked up
  /// by it.
  fn project_dirs(
      projects: RawFd,
      base: &Path,
      dev: libc::dev_t,
      tally: &mut Tally,
  ) -> Vec<(OwnedFd, std::ffi::CString)> {
      let names = match walk::list(projects) {
          Ok(names) => names,
          Err(_) => {
              tally.left(ForgetKind::Transcript, ForgetReason::IoError);
              return Vec::new();
          }
      };
      let mut dirs = Vec::new();
      for name in names {
          let _path = base.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
          match walk::open_dir_at(projects, &name) {
              Ok(fd) => match walk::stat_fd(fd.as_raw_fd()) {
                  Ok(st) if st.st_dev == dev => dirs.push((fd, name)),
                  Ok(_) => tally.left(ForgetKind::Transcript, ForgetReason::MountPoint),
                  Err(_) => tally.left(ForgetKind::Transcript, ForgetReason::IoError),
              },
              Err(libc::ELOOP | libc::ENOTDIR | libc::ENOENT) => {}
              Err(_) => tally.left(ForgetKind::Transcript, ForgetReason::IoError),
          }
      }
      dirs
  }

  /// The exact entries of `kinds` (B9), each through descriptors (R1, R2),
  /// then the check afterwards (B4): what is still there is what is left,
  /// whatever the removal reported.
  fn remove_and_verify(kinds: &Kinds, root: &Path, id: &str, hooks: &walk::Hooks) -> Forgotten {
      let mut tally = Tally::default();
      // Why an entry was not removed, by kind and name, for the check after.
      let mut why: BTreeMap<(ForgetKind, Vec<u8>), ForgetReason> = BTreeMap::new();
      let mut each = |kind: ForgetKind, dir: RawFd, path: PathBuf, name: &[u8], tally: &mut Tally| {
          let c = walk::c_name(name);
          match walk::remove_entry(dir, &c, kinds.dev, &path, hooks) {
              walk::Removal::Absent => {}
              walk::Removal::Removed => *tally.removed.entry(kind).or_default() += 1,
              walk::Removal::Symlink => {
                  why.insert((kind, name.to_vec()), ForgetReason::Symlink);
              }
              walk::Removal::Stopped(stop) => {
                  why.insert((kind, name.to_vec()), stop_reason(stop));
              }
          }
      };
      for (kind, dir_name, opened) in &kinds.dirs {
          let dir = match opened {
              Ok(Some(fd)) => fd.as_raw_fd(),
              Ok(None) => continue,
              Err(reason) => {
                  tally.left(*kind, *reason);
                  continue;
              }
          };
          let base = root.join(dir_name);
          if *kind == ForgetKind::Transcript {
              for (project, project_name) in project_dirs(dir, &base, kinds.dev, &mut tally) {
                  let project_path = base.join(std::ffi::OsStr::from_bytes(project_name.as_bytes()));
                  let names = walk::list(project.as_raw_fd()).unwrap_or_default();
                  for name in names.iter().filter(|n| in_transcript_family(n.as_bytes(), id)) {
                      let path = project_path.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
                      each(*kind, project.as_raw_fd(), path, name.as_bytes(), &mut tally);
                  }
              }
          } else {
              let name = kind_entry(*kind, id);
              each(*kind, dir, base.join(&name), name.as_bytes(), &mut tally);
          }
      }
      // The check afterwards (B4).
      let verify =
          |kind: ForgetKind, dir: RawFd, name: &[u8], tally: &mut Tally| match walk::stat_at(dir, &walk::c_name(name)) {
              Ok(None) => {}
              Ok(Some(st)) if walk::is_link(&st) => tally.left(kind, ForgetReason::Symlink),
              Ok(Some(_)) => {
                  let reason = why
                      .get(&(kind, name.to_vec()))
                      .copied()
                      .unwrap_or(ForgetReason::StillPresent);
                  tally.left(kind, reason);
              }
              Err(_) => tally.left(kind, ForgetReason::IoError),
          };
      for (kind, _, opened) in &kinds.dirs {
          let Ok(Some(fd)) = opened else {
              continue;
          };
          if *kind == ForgetKind::Transcript {
              let mut scratch = Tally::default();
              for (project, _) in project_dirs(fd.as_raw_fd(), Path::new(""), kinds.dev, &mut scratch) {
                  let names = walk::list(project.as_raw_fd()).unwrap_or_default();
                  for name in names.iter().filter(|n| in_transcript_family(n.as_bytes(), id)) {
                      verify(*kind, project.as_raw_fd(), name.as_bytes(), &mut tally);
                  }
              }
          } else {
              verify(*kind, fd.as_raw_fd(), kind_entry(*kind, id).as_bytes(), &mut tally);
          }
      }
      let _ = &kinds.root;
      tally.into_forgotten()
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      use super::*;

  ```

  with:

  ```rust
      use super::*;

      #[test]
      fn only_the_transcripts_own_names_are_its_family() {
          let id = "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";
          for suffix in [
              ".jsonl",
              "",
              ".ccr-tip.json",
              ".precompact.json",
              ".cast",
              ".dir-sync.json",
              ".dir-sync-empty.json",
              ".jsonl.superseded-1",
              ".jsonl.compact.tmp.x",
          ] {
              assert!(
                  in_transcript_family(format!("{id}{suffix}").as_bytes(), id),
                  "{suffix:?}"
              );
          }
          for name in [
              format!("{id}x.jsonl"),
              format!("{id}.jsonl.bak"),
              format!("{id}.json"),
              format!("x{id}.jsonl"),
              "other.jsonl".to_string(),
          ] {
              assert!(!in_transcript_family(name.as_bytes(), id), "{name:?}");
          }
      }

  ```

  In `crates/hennery-host/src/lib.rs`, replace:

  ```rust
  pub mod uplink;

  ```

  with:

  ```rust
  pub mod uplink;
  pub mod walk;

  ```

  Create `crates/hennery-host/src/walk.rs`:

  ```rust
  //! Removing an agent's entries through directory descriptors (plan 9d B3,
  //! R1, R2): every step is relative to a directory already open, opened
  //! with `O_NOFOLLOW`, so no path component is looked up twice and no
  //! symlink, however it is swapped in, is ever followed.
  //!
  //! - A directory is opened with `openat(O_RDONLY|O_DIRECTORY|O_NOFOLLOW|
  //!   O_CLOEXEC|O_NONBLOCK)`; `ELOOP` or `ENOTDIR` means it is no directory,
  //!   and it is unlinked as an entry (`unlinkat(…, 0)`), a symlink included:
  //!   its target is never touched. The entry type `readdir` reports is
  //!   never trusted: every entry is tried as a directory first.
  //! - Entries are read through `fdopendir` on a `dup` of the directory's
  //!   descriptor; a directory goes with `unlinkat(AT_REMOVEDIR)` after its
  //!   contents.
  //! - `EINTR` is retried; `ENOENT` means gone already.
  //! - The walk is iterative, at most `MAX_DEPTH` directories deep, one
  //!   descriptor per level; it stops at another file system (`st_dev`).

  use std::ffi::{CStr, CString};
  use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
  use std::os::unix::ffi::OsStrExt;
  use std::path::{Path, PathBuf};

  /// The deepest a removal descends (R2): one open descriptor per level.
  pub const MAX_DEPTH: usize = 32;

  /// Why a removal stopped short (R2, B3).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Stop {
      /// A directory on another file system (a mount point).
      MountPoint,
      /// Deeper than `MAX_DEPTH`.
      TooDeep,
      /// A system call failed with this errno.
      Io(i32),
  }

  /// Seams for tests (the host crate's `test-hooks` feature): nothing in a
  /// real build.
  #[derive(Clone, Default)]
  pub struct Hooks {
      /// Called after each directory of a removal has been listed, before any
      /// of its entries is acted on, with its path (for the test to find it;
      /// the walk itself never uses a path) and its entries.
      #[cfg(feature = "test-hooks")]
      #[allow(clippy::type_complexity)]
      pub listed: Option<std::sync::Arc<dyn Fn(&Path, &[std::ffi::OsString]) + Send + Sync>>,
  }

  impl std::fmt::Debug for Hooks {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.write_str("Hooks")
      }
  }

  impl Hooks {
      fn listed(&self, _path: &Path, _entries: &[CString]) {
          #[cfg(feature = "test-hooks")]
          if let Some(hook) = &self.listed {
              use std::os::unix::ffi::OsStringExt;
              let names: Vec<std::ffi::OsString> = _entries
                  .iter()
                  .map(|n| std::ffi::OsString::from_vec(n.as_bytes().to_vec()))
                  .collect();
              hook(_path, &names);
          }
      }
  }

  fn errno() -> i32 {
      std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)
  }

  /// Clear `errno`, so a `NULL` from `readdir` can be told from an error.
  fn clear_errno() {
      // SAFETY: the calling thread's own errno slot.
      unsafe {
          #[cfg(any(target_os = "macos", target_os = "ios"))]
          {
              *libc::__error() = 0;
          }
          #[cfg(not(any(target_os = "macos", target_os = "ios")))]
          {
              *libc::__errno_location() = 0;
          }
      }
  }

  /// `name` as a C string: no name read from a directory or built from a
  /// checked id holds a NUL.
  pub fn c_name(name: &[u8]) -> CString {
      CString::new(name).expect("a file name holds no NUL")
  }

  const DIR_FLAGS: libc::c_int =
      libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;

  /// Open `path` as a directory, never through a symlink at its last
  /// component: the agent's root, which the caller has checked is canonical.
  pub fn open_root(path: &Path) -> Result<OwnedFd, i32> {
      let path = c_name(path.as_os_str().as_bytes());
      loop {
          // SAFETY: open(2) on a NUL-terminated path.
          let fd = unsafe { libc::open(path.as_ptr(), DIR_FLAGS) };
          if fd >= 0 {
              // SAFETY: a descriptor this call just opened, owned from here.
              return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
          }
          match errno() {
              libc::EINTR => continue,
              e => return Err(e),
          }
      }
  }

  /// Open the directory `name` in `dir`, never following a symlink (R1):
  /// `ELOOP` or `ENOTDIR` if it is no directory.
  pub fn open_dir_at(dir: RawFd, name: &CStr) -> Result<OwnedFd, i32> {
      loop {
          // SAFETY: openat(2) relative to an open directory descriptor.
          let fd = unsafe { libc::openat(dir, name.as_ptr(), DIR_FLAGS) };
          if fd >= 0 {
              // SAFETY: as in `open_root`.
              return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
          }
          match errno() {
              libc::EINTR => continue,
              e => return Err(e),
          }
      }
  }

  /// `fstat` of an open descriptor.
  pub fn stat_fd(fd: RawFd) -> Result<libc::stat, i32> {
      // SAFETY: an all-zero `stat` is a valid value for the call to fill.
      let mut st: libc::stat = unsafe { std::mem::zeroed() };
      // SAFETY: fstat(2) into a local struct.
      if unsafe { libc::fstat(fd, &mut st) } == 0 {
          Ok(st)
      } else {
          Err(errno())
      }
  }

  /// `fstatat(AT_SYMLINK_NOFOLLOW)` of `name` in `dir`; `None` if it is not
  /// there.
  pub fn stat_at(dir: RawFd, name: &CStr) -> Result<Option<libc::stat>, i32> {
      loop {
          // SAFETY: as in `stat_fd`.
          let mut st: libc::stat = unsafe { std::mem::zeroed() };
          // SAFETY: fstatat(2) relative to an open directory, into a local.
          if unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } == 0 {
              return Ok(Some(st));
          }
          match errno() {
              libc::EINTR => continue,
              libc::ENOENT => return Ok(None),
              e => return Err(e),
          }
      }
  }

  pub fn is_link(st: &libc::stat) -> bool {
      st.st_mode & libc::S_IFMT == libc::S_IFLNK
  }

  pub fn is_dir(st: &libc::stat) -> bool {
      st.st_mode & libc::S_IFMT == libc::S_IFDIR
  }

  /// Unlink `name` in `dir`; gone already is fine.
  fn unlink_at(dir: RawFd, name: &CStr, flags: libc::c_int) -> Result<(), i32> {
      loop {
          // SAFETY: unlinkat(2) relative to an open directory descriptor.
          if unsafe { libc::unlinkat(dir, name.as_ptr(), flags) } == 0 {
              return Ok(());
          }
          match errno() {
              libc::EINTR => continue,
              libc::ENOENT => return Ok(()),
              e => return Err(e),
          }
      }
  }

  /// The entries of the open directory `dir`, `.` and `..` left out, read
  /// through `fdopendir` on a `dup` of it (R1), which is closed after.
  pub fn list(dir: RawFd) -> Result<Vec<CString>, i32> {
      // SAFETY: dup(2) of an open descriptor; fdopendir(3) takes the copy,
      // and closedir(3) closes it.
      let copy = unsafe { libc::fcntl(dir, libc::F_DUPFD_CLOEXEC, 0) };
      if copy < 0 {
          return Err(errno());
      }
      // SAFETY: as above.
      let stream = unsafe { libc::fdopendir(copy) };
      if stream.is_null() {
          let e = errno();
          // SAFETY: the copy is still ours when fdopendir failed.
          unsafe { libc::close(copy) };
          return Err(e);
      }
      // The stream reads from the copy's offset, which it shares with `dir`:
      // start from the beginning whatever was read before.
      // SAFETY: rewinddir(3) on the stream just opened.
      unsafe { libc::rewinddir(stream) };
      let mut out = Vec::new();
      let result = loop {
          clear_errno();
          // SAFETY: readdir(3) on a live stream; the entry is copied out
          // before the next call.
          let entry = unsafe { libc::readdir(stream) };
          if entry.is_null() {
              match errno() {
                  0 => break Ok(()),
                  libc::EINTR => continue,
                  e => break Err(e),
              }
          }
          // SAFETY: `d_name` is NUL-terminated within the entry (its length
          // differs by platform: 256 bytes on Linux, 1024 on macOS).
          let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
          let bytes = name.to_bytes();
          if bytes != b"." && bytes != b".." {
              out.push(name.to_owned());
          }
      };
      // SAFETY: closes the stream and the copy it owns.
      unsafe { libc::closedir(stream) };
      result.map(|()| out)
  }

  /// One directory being removed.
  struct Frame {
      fd: OwnedFd,
      /// Its name in its parent.
      name: CString,
      entries: Vec<CString>,
      next: usize,
      /// For the hooks only.
      path: PathBuf,
  }

  /// Remove the directory `name` in `parent` and everything in it (R1, R2),
  /// on the file system `dev`. An entry that turns out not to be a directory
  /// (a symlink swapped in included) is unlinked as an entry.
  pub fn remove_tree(parent: RawFd, name: &CStr, dev: libc::dev_t, path: &Path, hooks: &Hooks) -> Result<(), Stop> {
      let mut stack: Vec<Frame> = Vec::new();
      match descend(parent, name, dev, path, hooks)? {
          Some(frame) => stack.push(frame),
          None => return Ok(()),
      }
      while let Some(top) = stack.last_mut() {
          if top.next < top.entries.len() {
              let child = top.entries[top.next].clone();
              top.next += 1;
              let child_path = top.path.join(std::ffi::OsStr::from_bytes(child.as_bytes()));
              let fd = top.fd.as_raw_fd();
              if stack.len() >= MAX_DEPTH {
                  // Only a directory makes it deeper.
                  if matches!(stat_at(fd, &child), Ok(Some(st)) if is_dir(&st)) {
                      return Err(Stop::TooDeep);
                  }
              }
              if let Some(frame) = descend(fd, &child, dev, &child_path, hooks)? {
                  stack.push(frame);
              }
          } else {
              let done = stack.pop().expect("a frame on the stack");
              let parent_fd = stack.last().map_or(parent, |f| f.fd.as_raw_fd());
              drop(done.fd);
              unlink_at(parent_fd, &done.name, libc::AT_REMOVEDIR).map_err(Stop::Io)?;
          }
      }
      Ok(())
  }

  /// Open `name` in `dir` to remove what it holds: its frame, listed, if it
  /// is a directory on `dev`; `None` once it is unlinked (no directory) or
  /// gone.
  fn descend(dir: RawFd, name: &CStr, dev: libc::dev_t, path: &Path, hooks: &Hooks) -> Result<Option<Frame>, Stop> {
      let fd = match open_dir_at(dir, name) {
          Ok(fd) => fd,
          Err(libc::ELOOP | libc::ENOTDIR) => {
              unlink_at(dir, name, 0).map_err(Stop::Io)?;
              return Ok(None);
          }
          Err(libc::ENOENT) => return Ok(None),
          Err(e) => return Err(Stop::Io(e)),
      };
      let st = stat_fd(fd.as_raw_fd()).map_err(Stop::Io)?;
      if st.st_dev != dev {
          return Err(Stop::MountPoint);
      }
      let entries = list(fd.as_raw_fd()).map_err(Stop::Io)?;
      hooks.listed(path, &entries);
      Ok(Some(Frame {
          fd,
          name: name.to_owned(),
          entries,
          next: 0,
          path: path.to_path_buf(),
      }))
  }

  /// What became of one named entry.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Removal {
      /// It was not there.
      Absent,
      Removed,
      /// A symlink: reported, never followed or removed (decision 8).
      Symlink,
      Stopped(Stop),
  }

  /// Remove the entry `name` in `dir`, which the forget names exactly (B9):
  /// a symlink is left and reported; a directory goes with its contents; any
  /// other entry is unlinked.
  pub fn remove_entry(dir: RawFd, name: &CStr, dev: libc::dev_t, path: &Path, hooks: &Hooks) -> Removal {
      match stat_at(dir, name) {
          Ok(None) => Removal::Absent,
          Ok(Some(st)) if is_link(&st) => Removal::Symlink,
          Ok(Some(st)) if is_dir(&st) => match remove_tree(dir, name, dev, path, hooks) {
              Ok(()) => Removal::Removed,
              Err(stop) => Removal::Stopped(stop),
          },
          Ok(Some(_)) => match unlink_at(dir, name, 0) {
              Ok(()) => Removal::Removed,
              Err(e) => Removal::Stopped(Stop::Io(e)),
          },
          Err(e) => Removal::Stopped(Stop::Io(e)),
      }
  }
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
      AgentCapabilities, CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk, InitializeRequest,
      InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse,
      PromptCapabilities, PromptRequest, PromptResponse, SessionConfigKind, SessionConfigOption,
      SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelect, SessionConfigSelectOption,
      SessionConfigSelectOptions, SessionConfigValueId, SessionId, SessionNotification, SessionUpdate,
      SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason, TextContent,
  ```

  with:

  ```rust
      AgentCapabilities, CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk, DeleteSessionRequest,
      DeleteSessionResponse, InitializeRequest, InitializeResponse, LoadSessionRequest, LoadSessionResponse,
      NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse, SessionCapabilities,
      SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelect,
      SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId, SessionDeleteCapabilities, SessionId,
      SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
      TextContent,
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
                                  .load_session(load_session)
                                  .prompt_capabilities(PromptCapabilities::new().image(images)),
  ```

  with:

  ```rust
                                  .load_session(load_session)
                                  .session_capabilities(
                                      SessionCapabilities::new().delete(SessionDeleteCapabilities::new()),
                                  )
                                  .prompt_capabilities(PromptCapabilities::new().image(images)),
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
          )
          .on_receive_notification(
  ```

  with:

  ```rust
          )
          .on_receive_request(
              {
                  let script = script.clone();
                  async move |req: DeleteSessionRequest, responder, cx| {
                      let script = script.clone();
                      cx.spawn(async move {
                          let deleted = delete_session(&script, &req.session_id);
                          if let Some(gate) = &script.delete_waits_for_file {
                              while !std::path::Path::new(gate).exists() {
                                  tokio::time::sleep(Duration::from_millis(10)).await;
                              }
                          }
                          match deleted {
                              Ok(()) => responder.respond(DeleteSessionResponse::new()),
                              Err(message) => {
                                  responder.respond_with_error(agent_client_protocol::Error::new(-32603, message))
                              }
                          }
                      })
                  }
              },
              agent_client_protocol::on_receive_request!(),
          )
          .on_receive_notification(
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
  /// apply either way.
  fn answer_load(
  ```

  with:

  ```rust
  /// apply either way.
  /// `session/delete` as Claude's SDK runs it (`FakeScript::delete_log`):
  /// the first project directory under `$CLAUDE_CONFIG_DIR/projects` with a
  /// non-empty `<id>.jsonl` loses it and the `<id>/` beside it, through
  /// paths (links followed, as the SDK's `fs` calls do); none is an error.
  fn delete_session(script: &FakeScript, session: &SessionId) -> Result<(), String> {
      let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "-".into());
      if let Some(log) = &script.delete_log {
          let cwd = std::env::current_dir()
              .map(|d| d.display().to_string())
              .unwrap_or_default();
          let mut file = std::fs::OpenOptions::new()
              .create(true)
              .append(true)
              .open(log)
              .expect("open delete_log");
          writeln!(
              file,
              "CLAUDE_CONFIG_DIR={}\ncwd={cwd}\nCLAUDE_CODE_PROJECT_DIR_NAME={}\nCODEX_SQLITE_HOME={}",
              var("CLAUDE_CONFIG_DIR"),
              var("CLAUDE_CODE_PROJECT_DIR_NAME"),
              var("CODEX_SQLITE_HOME"),
          )
          .expect("write delete_log");
      }
      let Some(root) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) else {
          return Err("no CLAUDE_CONFIG_DIR".into());
      };
      let id = session.to_string();
      let projects = std::path::Path::new(&root).join("projects");
      for dir in std::fs::read_dir(projects).into_iter().flatten().flatten() {
          let transcript = dir.path().join(format!("{id}.jsonl"));
          if std::fs::metadata(&transcript).is_ok_and(|m| m.len() > 0) {
              let _ = std::fs::remove_file(&transcript);
              let _ = std::fs::remove_dir_all(dir.path().join(&id));
              return Ok(());
          }
      }
      Err(format!("Session {id} not found in any project directory"))
  }

  fn answer_load(
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
      pub session_id: Option<String>,
  }
  ```

  with:

  ```rust
      pub session_id: Option<String>,
      /// On `session/delete` (advertised as `sessionCapabilities.delete`),
      /// append the environment and cwd it ran with to this file, one
      /// `name=value` line each: `CLAUDE_CONFIG_DIR`, `cwd`,
      /// `CLAUDE_CODE_PROJECT_DIR_NAME` and `CODEX_SQLITE_HOME` (`-` when
      /// unset). The delete itself acts as Claude's SDK does: it removes
      /// `<CLAUDE_CONFIG_DIR>/projects/*/<id>.jsonl` (following links) and
      /// the `<id>/` beside it, and answers an error when there is none. With
      /// no `CLAUDE_CONFIG_DIR` it removes nothing: the fake never touches a
      /// real home.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub delete_log: Option<String>,
      /// Answer `session/delete` only once this file exists, from a task of
      /// its own (its `delete_log` lines are written on receipt): a forget
      /// held in flight for exactly as long as the test says.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub delete_waits_for_file: Option<String>,
  }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
              session_id: None,
          }
  ```

  with:

  ```rust
              session_id: None,
              delete_log: None,
              delete_waits_for_file: None,
          }
  ```


- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-host -p hennery-testkit --locked --features hennery-host/test-hooks`
Expected: PASS, also under `umask 002`.

- [ ] **Step 5: Revert-probes**

- R3's three tests each fail when the descriptor call is replaced by a path-based one that follows links.
- The id check, the registry match, and each root and kind-directory check were probed.
- The adapter's environment (B6) was probed.
- The live-actor check and the attach marker were probed.

- [ ] **Step 6: The full checks**

- [ ] **Step 7: Commit**

```bash
git -c commit.gpgsign=false commit -am "feat(host): remove a deleted Claude session's transcript through directory descriptors"
```

### Task 4: The walk hardened, and one test per outcome (9d-i-b)

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
              hooks,
          }
  ```

  with:

  ```rust
              hooks,
              // Looked up by the forget itself, as the host's connection does.
              account: None,
          }
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
      assert!(std::fs::symlink_metadata(root.at(&format!("file-history/{ID}"))).is_ok());
      assert!(std::fs::symlink_metadata(root.at(&format!("projects/-p/{ID}.cast"))).is_ok());
  ```

  with:

  ```rust
      assert!(std::fs::symlink_metadata(root.at(&format!("file-history/{ID}"))).is_ok());
      // Something left: a partial outcome.
      let HostFrame::SessionForgotten { outcome, .. } = forgotten.clone().into_frame("r".into()) else {
          unreachable!()
      };
      assert_eq!(outcome, ForgetOutcome::Partial);
      assert!(std::fs::symlink_metadata(root.at(&format!("projects/-p/{ID}.cast"))).is_ok());
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
          })),
      };
  ```

  with:

  ```rust
          })),
          ..Hooks::default()
      };
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
      std::fs::set_permissions(root.root(), std::fs::Permissions::from_mode(0o775)).unwrap();
  ```

  with:

  ```rust
      // Writable by others: refused on every platform. (Group-writable passes
      // for the user's private group, as on ubuntu's runners, and fails for a
      // shared one, as macOS's `staff`: its own test covers both.)
      std::fs::set_permissions(root.root(), std::fs::Permissions::from_mode(0o757)).unwrap();
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
      let forgotten = forget(&ctx, &root.forget_at(&root.root())).await;
      assert_eq!(reasons(&forgotten), []);
      assert_eq!(
  ```

  with:

  ```rust
      let forgotten = forget(&ctx, &root.forget_at(&root.root())).await;
      // Counted, never followed (the review's item 4).
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Transcript, ForgetReason::Symlink, false)]
      );
      assert_eq!(forgotten.remaining[0].what.count, 1);
      assert_eq!(
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
      assert!(locked.join("f").exists());
  }
  ```

  with:

  ```rust
      assert!(locked.join("f").exists());
  }

  /// The review's item 8: a named directory swapped for a symlink after it
  /// was looked at is reported as a symlink, and neither it nor its target
  /// is removed.
  #[tokio::test]
  async fn a_named_directory_swapped_for_a_symlink_is_reported_not_unlinked() {
      let root = Root::new();
      std::fs::create_dir_all(root.outside().join("target")).unwrap();
      std::fs::write(root.outside().join("target/precious"), "keep").unwrap();
      let named = root.at(&format!("tasks/{ID}"));
      std::fs::create_dir_all(&named).unwrap();
      let (target, aside) = (root.outside().join("target"), root.outside().join("aside"));
      let at = named.clone();
      let hooks = Hooks {
          stated: Some(Arc::new(move |path: &Path| {
              if path == at {
                  std::fs::rename(path, &aside).unwrap();
                  symlink(&target, path).unwrap();
              }
          })),
          ..Hooks::default()
      };
      let forgotten = forget(&root.ctx(hooks), &root.forget_at(&root.root())).await;
      assert_eq!(
          std::fs::read_to_string(root.outside().join("target/precious")).unwrap(),
          "keep"
      );
      assert!(std::fs::symlink_metadata(&named).unwrap().file_type().is_symlink());
      assert_eq!(reasons(&forgotten), [(ForgetKind::Tasks, ForgetReason::Symlink, false)]);
  }

  /// The review's item 1: a project directory whose listing fails is left
  /// for a retry, counted once, in either pass.
  #[tokio::test]
  async fn a_project_directory_that_cannot_be_listed_is_left_for_a_retry() {
      use std::sync::atomic::{AtomicUsize, Ordering};
      // Fails the `fail_on`-th listing of `-p` (0: every one).
      async fn run(fail_on: usize) -> (Root, Forgotten) {
          let root = Root::new();
          std::fs::create_dir_all(root.at("projects/-p")).unwrap();
          std::fs::write(root.at(&format!("projects/-p/{ID}.cast")), "x").unwrap();
          let project = root.at("projects/-p");
          let calls = Arc::new(AtomicUsize::new(0));
          let hooks = Hooks {
              fail_listing: Some(Arc::new(move |path: &Path| {
                  path == project && {
                      let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                      fail_on == 0 || n == fail_on
                  }
              })),
              ..Hooks::default()
          };
          let mut ctx = root.ctx(hooks);
          ctx.agents.clear();
          let forgotten = forget(&ctx, &root.forget_at(&root.root())).await;
          (root, forgotten)
      }
      let (root, always) = run(0).await;
      assert_eq!(
          reasons(&always),
          [(ForgetKind::Transcript, ForgetReason::IoError, true)]
      );
      assert_eq!(always.remaining[0].what.count, 1);
      assert!(root.at(&format!("projects/-p/{ID}.cast")).exists());
      // Only the removal's listing fails: the check after finds it still there.
      let (_root, first) = run(1).await;
      assert_eq!(
          reasons(&first),
          [
              (ForgetKind::Transcript, ForgetReason::StillPresent, true),
              (ForgetKind::Transcript, ForgetReason::IoError, true),
          ]
      );
      // Only the check's listing fails: what it cannot see is not called gone.
      let (root, second) = run(2).await;
      assert_eq!(
          reasons(&second),
          [(ForgetKind::Transcript, ForgetReason::IoError, true)]
      );
      assert!(!root.at(&format!("projects/-p/{ID}.cast")).exists());
  }

  /// The review's item 5 end to end: with a umask of 002 the host user's
  /// directories are group-writable; they pass only where that group is the
  /// user's own private one (an Ubuntu runner), never a shared one (macOS's
  /// `staff`).
  #[tokio::test]
  async fn a_group_writable_root_passes_only_for_a_private_group() {
      let root = Root::new();
      std::fs::create_dir_all(root.at(&format!("tasks/{ID}"))).unwrap();
      for dir in [root.root(), root.at("tasks")] {
          std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o775)).unwrap();
      }
      use std::os::unix::fs::MetadataExt;
      let gid = std::fs::metadata(root.root()).unwrap().gid();
      let account = hennery_host::forget::account();
      let private = gid == account.gid
          && hennery_host::forget::group(gid)
              .is_some_and(|g| g.name == account.name && (g.members.is_empty() || g.members == [account.name.clone()]));
      let forgotten = root.forget().await;
      if private {
          assert_eq!(reasons(&forgotten), []);
          assert!(!root.at(&format!("tasks/{ID}")).exists());
      } else {
          assert_eq!(
              reasons(&forgotten),
              [(ForgetKind::Session, ForgetReason::UnsafeRoot, false)]
          );
          assert!(root.at(&format!("tasks/{ID}")).exists());
      }
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-host --locked --features test-hooks walk forget`
Expected: FAIL, among them `a_project_directory_that_cannot_be_listed_is_left_for_a_retry` and `only_the_users_private_group_may_write_a_directory_besides_the_user`.

- [ ] **Step 3: The hardening**

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          hooks: crate::walk::Hooks::default(),
      };
  ```

  with:

  ```rust
          hooks: crate::walk::Hooks::default(),
          // Looked up in the forget's blocking task, not here.
          account: None,
      };
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      pub hooks: crate::walk::Hooks,
  }
  ```

  with:

  ```rust
      pub hooks: crate::walk::Hooks,
      /// The host user, for the ownership checks (B3). `None`: looked up
      /// (`account`) inside the forget's blocking task, never on the
      /// connection's frame handler, where a slow directory service (LDAP,
      /// sssd) would stall the connection loop.
      pub account: Option<Account>,
  }
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      if kinds.dirs[0].2.is_ok() {
          run_adapter(ctx, forget, &root, until).await;
      }
      let (id, hooks) = (forget.agent_session_id.clone(), ctx.hooks.clone());
      match tokio::task::spawn_blocking(move || remove_and_verify(&kinds, &root, &id, &hooks)).await {
  ```

  with:

  ```rust
      // Not with a `projects/` that failed its check: the adapter would
      // follow it (B3).
      let projects_ok = kinds
          .dirs
          .iter()
          .any(|(kind, _, opened)| *kind == ForgetKind::Transcript && opened.is_ok());
      if projects_ok {
          run_adapter(ctx, forget, &root, until).await;
      }
      let (id, hooks) = (forget.agent_session_id.clone(), ctx.hooks.clone());
      let until = until.into_std();
      match tokio::task::spawn_blocking(move || remove_and_verify(&kinds, &root, &id, &hooks, until)).await {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  /// The host user's own, and not writable by group or others (B3).
  fn safely_owned(st: &libc::stat) -> bool {
      // SAFETY: geteuid(2) cannot fail.
      st.st_uid == unsafe { libc::geteuid() } && st.st_mode & 0o022 == 0
  ```

  with:

  ```rust
  /// The host user, as the safety checks need them (B3): the effective uid,
  /// and its account's primary gid and name (`getpwuid_r`). With no account
  /// the name is empty and the gid matches nothing.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Account {
      pub uid: libc::uid_t,
      pub gid: libc::gid_t,
      pub name: Vec<u8>,
  }

  /// A group, as `getgrgid_r` gives it.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Group {
      pub gid: libc::gid_t,
      pub name: Vec<u8>,
      pub members: Vec<Vec<u8>>,
  }

  /// Whether a root or kind directory with `st` is safe to remove from (B3;
  /// the review's item 5): the host user's, never writable by others, and
  /// writable by its group only when that group is the user's own private
  /// one: the account's primary group, named as the user, with no member
  /// but the user (a umask of 002 makes every directory so). A shared group
  /// (`staff`) does not count.
  pub fn safe_mode(st: &libc::stat, account: &Account, group: Option<&Group>) -> bool {
      if st.st_uid != account.uid || st.st_mode & 0o002 != 0 {
          return false;
      }
      if st.st_mode & 0o020 == 0 {
          return true;
      }
      let Some(group) = group else {
          return false;
      };
      !account.name.is_empty()
          && st.st_gid == account.gid
          && group.gid == st.st_gid
          && group.name == account.name
          && (group.members.is_empty() || group.members == [account.name.clone()])
  }

  /// The size of a buffer for `getpwuid_r` / `getgrgid_r`, and its growth.
  const LOOKUP_BUFFER: usize = 1024;
  const LOOKUP_BUFFER_MAX: usize = 1 << 20;

  /// The host user's account (`getpwuid_r` of the effective uid), only the
  /// reentrant call.
  pub fn account() -> Account {
      // SAFETY: geteuid(2) cannot fail.
      let uid = unsafe { libc::geteuid() };
      let mut size = LOOKUP_BUFFER;
      while size <= LOOKUP_BUFFER_MAX {
          let mut buf = vec![0 as libc::c_char; size];
          // SAFETY: an all-zero `passwd` is a valid value for the call to fill.
          let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
          let mut found: *mut libc::passwd = std::ptr::null_mut();
          // SAFETY: getpwuid_r(3) into local storage of the given size.
          let rc = unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut found) };
          match rc {
              0 if !found.is_null() => {
                  // SAFETY: `pw_name` points into `buf`, NUL-terminated.
                  let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_bytes().to_vec();
                  return Account {
                      uid,
                      gid: pwd.pw_gid,
                      name,
                  };
              }
              libc::ERANGE => size *= 2,
              libc::EINTR => {}
              _ => break,
          }
      }
      Account {
          uid,
          gid: libc::gid_t::MAX,
          name: Vec::new(),
      }
  }

  /// The group `gid` (`getgrgid_r`), if there is one.
  pub fn group(gid: libc::gid_t) -> Option<Group> {
      let mut size = LOOKUP_BUFFER;
      while size <= LOOKUP_BUFFER_MAX {
          let mut buf = vec![0 as libc::c_char; size];
          // SAFETY: as in `account`.
          let mut grp: libc::group = unsafe { std::mem::zeroed() };
          let mut found: *mut libc::group = std::ptr::null_mut();
          // SAFETY: getgrgid_r(3) into local storage of the given size.
          let rc = unsafe { libc::getgrgid_r(gid, &mut grp, buf.as_mut_ptr(), buf.len(), &mut found) };
          match rc {
              0 if !found.is_null() => {
                  // SAFETY: the name and the NULL-terminated member list point
                  // into `buf`.
                  let name = unsafe { std::ffi::CStr::from_ptr(grp.gr_name) }.to_bytes().to_vec();
                  let mut members = Vec::new();
                  let mut at = grp.gr_mem;
                  // SAFETY: as above; the list ends with a NULL pointer.
                  unsafe {
                      while !at.is_null() && !(*at).is_null() {
                          members.push(std::ffi::CStr::from_ptr(*at).to_bytes().to_vec());
                          at = at.add(1);
                      }
                  }
                  return Some(Group {
                      gid: grp.gr_gid,
                      name,
                      members,
                  });
              }
              0 => return None,
              libc::ERANGE => size *= 2,
              libc::EINTR => {}
              _ => return None,
          }
      }
      None
  }

  /// `safe_mode` for `st`, looking its group up only if it matters.
  fn safely_owned(st: &libc::stat, account: &Account) -> bool {
      let group = if st.st_mode & 0o020 != 0 {
          group(st.st_gid)
      } else {
          None
      };
      safe_mode(st, account, group.as_ref())
  }

  /// `(st_dev, st_ino)` of `path`, following links.
  fn identity(path: &Path) -> Option<(u64, u64)> {
      use std::os::unix::fs::MetadataExt;
      std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  fn open_checked_root(ctx: &ForgetContext, root: &Path) -> Result<(OwnedFd, libc::dev_t), ForgetReason> {
  ```

  with:

  ```rust
  fn open_checked_root(ctx: &ForgetContext, root: &Path, me: &Account) -> Result<(OwnedFd, libc::dev_t), ForgetReason> {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      }
      let mut guarded: Vec<PathBuf> = vec![ctx.data_dir.clone()];
  ```

  with:

  ```rust
      }
      let fd = walk::open_root(root).map_err(|_| ForgetReason::UnsafeRoot)?;
      let st = walk::stat_fd(fd.as_raw_fd()).map_err(|_| ForgetReason::UnsafeRoot)?;
      // The field types differ by platform (`st_dev` is `i32` on macOS).
      #[allow(clippy::unnecessary_cast)]
      let opened = (st.st_dev as u64, st.st_ino as u64);
      // The directory opened is the one the path names now (the review's
      // item 9): nothing was swapped in between the checks and the open.
      if identity(root) != Some(opened) {
          return Err(ForgetReason::UnsafeRoot);
      }
      // Not the host's data directory, nor the user's home, nor one of
      // their ancestors (`/` too), compared by device and inode.
      let mut guarded: Vec<PathBuf> = vec![ctx.data_dir.clone()];
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
          // `root` is that directory, or one of its ancestors (`/` too).
          if guarded.starts_with(root) {
              return Err(ForgetReason::UnsafeRoot);
          }
      }
      let fd = walk::open_root(root).map_err(|_| ForgetReason::UnsafeRoot)?;
      let st = walk::stat_fd(fd.as_raw_fd()).map_err(|_| ForgetReason::UnsafeRoot)?;
      if !walk::is_dir(&st) || !safely_owned(&st) {
  ```

  with:

  ```rust
          if guarded.ancestors().any(|dir| identity(dir) == Some(opened)) {
              return Err(ForgetReason::UnsafeRoot);
          }
      }
      if !walk::is_dir(&st) || !safely_owned(&st, me) {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  fn open_kind(root: RawFd, name: &str, dev: libc::dev_t) -> Result<Option<OwnedFd>, ForgetReason> {
  ```

  with:

  ```rust
  fn open_kind(root: RawFd, name: &str, dev: libc::dev_t, account: &Account) -> Result<Option<OwnedFd>, ForgetReason> {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      if !safely_owned(&st) {
  ```

  with:

  ```rust
      if !safely_owned(&st, account) {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      matches!(reason, ForgetReason::StillPresent | ForgetReason::IoError)
  ```

  with:

  ```rust
      matches!(
          reason,
          ForgetReason::StillPresent | ForgetReason::IoError | ForgetReason::TimedOut | ForgetReason::InProgress
      )
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
          walk::Stop::Io(_) => ForgetReason::IoError,
  ```

  with:

  ```rust
          walk::Stop::Io(_) | walk::Stop::Swapped => ForgetReason::IoError,
          walk::Stop::Deadline => ForgetReason::TimedOut,
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  fn check(ctx: &ForgetContext, root: &Path) -> Result<Kinds, ForgetReason> {
      let (root_fd, dev) = open_checked_root(ctx, root)?;
      let dirs = CLAUDE_KINDS
          .iter()
          .map(|&(kind, name)| (kind, name, open_kind(root_fd.as_raw_fd(), name, dev)))
  ```

  with:

  ```rust
  /// Runs on a blocking thread: the account lookup with it.
  fn check(ctx: &ForgetContext, root: &Path) -> Result<Kinds, ForgetReason> {
      let me = ctx.account.clone().unwrap_or_else(account);
      let (root_fd, dev) = open_checked_root(ctx, root, &me)?;
      let dirs = CLAUDE_KINDS
          .iter()
          .map(|&(kind, name)| (kind, name, open_kind(root_fd.as_raw_fd(), name, dev, &me)))
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  /// `dev`, each open, with its name; a symlinked one is skipped, never
  /// followed (decision 8); one on another file system is counted.
  ```

  with:

  ```rust
  /// `dev`, each open, with its name; a symlinked one is counted and never
  /// followed (decision 8; the review's item 4); one on another file system
  /// is counted; a file there is no project directory.
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
              Err(libc::ELOOP | libc::ENOTDIR | libc::ENOENT) => {}
  ```

  with:

  ```rust
              Err(libc::ELOOP | libc::ENOTDIR) => {
                  if matches!(walk::stat_at(projects, &name), Ok(Some(st)) if walk::is_link(&st)) {
                      tally.left(ForgetKind::Transcript, ForgetReason::Symlink);
                  }
              }
              Err(libc::ENOENT) => {}
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  fn remove_and_verify(kinds: &Kinds, root: &Path, id: &str, hooks: &walk::Hooks) -> Forgotten {
      let mut tally = Tally::default();
  ```

  with:

  ```rust
  fn remove_and_verify(
      kinds: &Kinds,
      root: &Path,
      id: &str,
      hooks: &walk::Hooks,
      until: std::time::Instant,
  ) -> Forgotten {
      let mut tally = Tally::default();
      // Project directories whose listing failed, in either pass: each counts
      // once, as left for a retry (the review's item 1).
      let mut unlisted: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
          match walk::remove_entry(dir, &c, kinds.dev, &path, hooks) {
  ```

  with:

  ```rust
          match walk::remove_entry(dir, &c, kinds.dev, &path, hooks, until) {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
              for (project, project_name) in project_dirs(dir, &base, kinds.dev, &mut tally) {
                  let project_path = base.join(std::ffi::OsStr::from_bytes(project_name.as_bytes()));
                  let names = walk::list(project.as_raw_fd()).unwrap_or_default();
  ```

  with:

  ```rust
              // What the first pass finds of the project directories is found
              // again, and counted, by the check after.
              let mut scratch = Tally::default();
              for (project, project_name) in project_dirs(dir, &base, kinds.dev, &mut scratch) {
                  let project_path = base.join(std::ffi::OsStr::from_bytes(project_name.as_bytes()));
                  let names = match hooks.list(project.as_raw_fd(), &project_path) {
                      Ok(names) => names,
                      Err(_) => {
                          if unlisted.insert(project_name.as_bytes().to_vec()) {
                              tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                          }
                          continue;
                      }
                  };
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      for (kind, _, opened) in &kinds.dirs {
  ```

  with:

  ```rust
      for (kind, dir_name, opened) in &kinds.dirs {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
              let mut scratch = Tally::default();
              for (project, _) in project_dirs(fd.as_raw_fd(), Path::new(""), kinds.dev, &mut scratch) {
                  let names = walk::list(project.as_raw_fd()).unwrap_or_default();
  ```

  with:

  ```rust
              let base = root.join(dir_name);
              for (project, project_name) in project_dirs(fd.as_raw_fd(), &base, kinds.dev, &mut tally) {
                  let project_path = base.join(std::ffi::OsStr::from_bytes(project_name.as_bytes()));
                  let names = match hooks.list(project.as_raw_fd(), &project_path) {
                      Ok(names) => names,
                      Err(_) => {
                          if unlisted.insert(project_name.as_bytes().to_vec()) {
                              tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                          }
                          continue;
                      }
                  };
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      use super::*;

  ```

  with:

  ```rust
      use super::*;

      fn me() -> Account {
          Account {
              uid: 501,
              gid: 501,
              name: b"me".to_vec(),
          }
      }

      fn dir(uid: libc::uid_t, gid: libc::gid_t, mode: libc::mode_t) -> libc::stat {
          // SAFETY: an all-zero `stat` is a valid value.
          let mut st: libc::stat = unsafe { std::mem::zeroed() };
          st.st_uid = uid;
          st.st_gid = gid;
          st.st_mode = libc::S_IFDIR | mode;
          st
      }

      fn grp(gid: libc::gid_t, name: &str, members: &[&str]) -> Group {
          Group {
              gid,
              name: name.as_bytes().to_vec(),
              members: members.iter().map(|m| m.as_bytes().to_vec()).collect(),
          }
      }

      /// The review's item 5.
      #[test]
      fn only_the_users_private_group_may_write_a_directory_besides_the_user() {
          let me = me();
          // Not group-writable: only the owner counts.
          assert!(safe_mode(&dir(501, 20, 0o755), &me, None));
          assert!(!safe_mode(&dir(502, 501, 0o755), &me, None));
          // A private group (umask 002): the user's own, empty or just them.
          let private = grp(501, "me", &[]);
          assert!(safe_mode(&dir(501, 501, 0o775), &me, Some(&private)));
          assert!(safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(501, "me", &["me"]))));
          // A shared group, `staff` (gid 20).
          assert!(!safe_mode(
              &dir(501, 20, 0o775),
              &me,
              Some(&grp(20, "staff", &["me", "you"]))
          ));
          // A looked-up group that is not the directory's.
          assert!(!safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(600, "me", &[]))));
          // A group named as the user and only theirs, but not their
          // primary one.
          assert!(!safe_mode(&dir(501, 600, 0o775), &me, Some(&grp(600, "me", &[]))));
          // The primary gid, but named otherwise, or with another member.
          assert!(!safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(501, "dev", &[]))));
          assert!(!safe_mode(&dir(501, 501, 0o775), &me, Some(&grp(501, "me", &["you"]))));
          assert!(!safe_mode(
              &dir(501, 501, 0o775),
              &me,
              Some(&grp(501, "me", &["me", "you"]))
          ));
          // A group it could not look up, or an account with no name.
          assert!(!safe_mode(&dir(501, 501, 0o775), &me, None));
          let nameless = Account {
              name: Vec::new(),
              ..me.clone()
          };
          assert!(!safe_mode(&dir(501, 501, 0o775), &nameless, Some(&grp(501, "", &[]))));
          // Writable by others: never.
          assert!(!safe_mode(&dir(501, 501, 0o777), &me, Some(&private)));
          assert!(!safe_mode(&dir(501, 501, 0o757), &me, Some(&private)));
      }

      /// `account` and `group` find the host user (the reentrant calls).
      #[test]
      fn the_host_user_and_their_group_are_found() {
          let account = account();
          // SAFETY: geteuid(2) cannot fail.
          assert_eq!(account.uid, unsafe { libc::geteuid() });
          assert!(!account.name.is_empty());
          assert_eq!(super::group(account.gid).map(|g| g.gid), Some(account.gid));
      }

      /// The review's item 7: a kind directory on another device is refused.
      #[test]
      fn a_kind_directory_on_another_file_system_is_refused() {
          let root = tempfile::tempdir().unwrap();
          std::fs::create_dir(root.path().join("tasks")).unwrap();
          let fd = walk::open_root(root.path()).unwrap();
          let dev = walk::stat_fd(fd.as_raw_fd()).unwrap().st_dev;
          let me = account();
          assert!(matches!(open_kind(fd.as_raw_fd(), "tasks", dev, &me), Ok(Some(_))));
          assert_eq!(
              open_kind(fd.as_raw_fd(), "tasks", dev ^ 1, &me).err(),
              Some(ForgetReason::MountPoint)
          );
      }

      /// The review's item 3: what the deadline left, a retry may finish.
      #[test]
      fn a_removal_cut_by_its_deadline_is_retried() {
          assert_eq!(stop_reason(walk::Stop::Deadline), ForgetReason::TimedOut);
          assert!(retryable(ForgetReason::TimedOut));
          assert!(!retryable(ForgetReason::Symlink));
      }

      /// Every way the walk stops has its reason (the fleet's per-outcome
      /// rule), and only those a retry can change are retried.
      #[test]
      fn each_stop_of_the_walk_has_its_reason() {
          assert_eq!(stop_reason(walk::Stop::MountPoint), ForgetReason::MountPoint);
          assert_eq!(stop_reason(walk::Stop::TooDeep), ForgetReason::TooDeep);
          assert_eq!(stop_reason(walk::Stop::Io(libc::EIO)), ForgetReason::IoError);
          assert_eq!(stop_reason(walk::Stop::Swapped), ForgetReason::IoError);
          for (reason, retried) in [
              (ForgetReason::StillPresent, true),
              (ForgetReason::IoError, true),
              (ForgetReason::InProgress, true),
              (ForgetReason::TimedOut, true),
              (ForgetReason::MountPoint, false),
              (ForgetReason::TooDeep, false),
              (ForgetReason::UnsafeDirectory, false),
              (ForgetReason::NotADirectory, false),
              (ForgetReason::UnsafeRoot, false),
              (ForgetReason::RootMissing, false),
          ] {
              assert_eq!(retryable(reason), retried, "{reason:?}");
          }
      }

      /// A forget the connection did not check never builds a path from an
      /// id the agent never writes.
      #[tokio::test]
      async fn an_unchecked_invalid_id_is_refused_by_the_forget_itself() {
          let ctx = ForgetContext {
              agents: HashMap::new(),
              data_dir: PathBuf::from("/nonexistent-data"),
              home: None,
              hooks: walk::Hooks::default(),
              account: None,
          };
          let forgotten = forget(
              &ctx,
              &Forget {
                  agent: "claude".into(),
                  agent_session_id: "../escape".into(),
                  agent_home: hennery_proto::frames::AgentHome {
                      root: "/tmp".into(),
                      sqlite_root: None,
                  },
              },
          )
          .await;
          assert_eq!(
              forgotten.remaining,
              [left(ForgetKind::Session, 0, ForgetReason::InvalidId, false)]
          );
      }

  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
      Io(i32),
  }
  ```

  with:

  ```rust
      Io(i32),
      /// The forget's deadline passed (B6; the review's item 3).
      Deadline,
      /// The named entry was a directory when it was looked at and is not
      /// one by the time it is opened (a symlink swapped in): left alone
      /// (the review's item 8).
      Swapped,
  }
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
      pub listed: Option<std::sync::Arc<dyn Fn(&Path, &[std::ffi::OsString]) + Send + Sync>>,
  }

  ```

  with:

  ```rust
      pub listed: Option<std::sync::Arc<dyn Fn(&Path, &[std::ffi::OsString]) + Send + Sync>>,
      /// Called once a named entry has been found to be a directory, before
      /// it is opened, with its path.
      #[cfg(feature = "test-hooks")]
      pub stated: Option<PathHook>,
      /// Asked before a project directory is listed, with its path: `true`
      /// makes the listing fail (`EIO`).
      #[cfg(feature = "test-hooks")]
      pub fail_listing: Option<PathTest>,
  }

  /// A test hook given a path (`Hooks::stated`).
  pub type PathHook = std::sync::Arc<dyn Fn(&Path) + Send + Sync>;

  /// A test hook asked about a path (`Hooks::fail_listing`).
  pub type PathTest = std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync>;

  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
              hook(_path, &names);
          }
      }
  }
  ```

  with:

  ```rust
              hook(_path, &names);
          }
      }

      fn stated(&self, _path: &Path) {
          #[cfg(feature = "test-hooks")]
          if let Some(hook) = &self.stated {
              hook(_path);
          }
      }

      /// `list`, unless a test makes the listing of `_path` fail.
      pub fn list(&self, dir: RawFd, _path: &Path) -> Result<Vec<CString>, i32> {
          #[cfg(feature = "test-hooks")]
          if let Some(hook) = &self.fail_listing
              && hook(_path)
          {
              return Err(libc::EIO);
          }
          list(dir)
      }
  }
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
  /// on the file system `dev`. An entry that turns out not to be a directory
  /// (a symlink swapped in included) is unlinked as an entry.
  pub fn remove_tree(parent: RawFd, name: &CStr, dev: libc::dev_t, path: &Path, hooks: &Hooks) -> Result<(), Stop> {
      let mut stack: Vec<Frame> = Vec::new();
      match descend(parent, name, dev, path, hooks)? {
  ```

  with:

  ```rust
  /// on the file system `dev`, before `until`. Below the top, an entry that
  /// turns out not to be a directory (a symlink swapped in included) is
  /// unlinked as an entry; the top itself is left (`Stop::Swapped`).
  pub fn remove_tree(
      parent: RawFd,
      name: &CStr,
      dev: libc::dev_t,
      path: &Path,
      hooks: &Hooks,
      until: std::time::Instant,
  ) -> Result<(), Stop> {
      if std::time::Instant::now() >= until {
          return Err(Stop::Deadline);
      }
      let mut stack: Vec<Frame> = Vec::new();
      match descend(parent, name, dev, 1, path, hooks)? {
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
      while let Some(top) = stack.last_mut() {
          if top.next < top.entries.len() {
  ```

  with:

  ```rust
      while let Some(top) = stack.last_mut() {
          if std::time::Instant::now() >= until {
              return Err(Stop::Deadline);
          }
          if top.next < top.entries.len() {
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
              if stack.len() >= MAX_DEPTH {
                  // Only a directory makes it deeper.
                  if matches!(stat_at(fd, &child), Ok(Some(st)) if is_dir(&st)) {
                      return Err(Stop::TooDeep);
                  }
              }
              if let Some(frame) = descend(fd, &child, dev, &child_path, hooks)? {
  ```

  with:

  ```rust
              let depth = stack.len() + 1;
              if let Some(frame) = descend(fd, &child, dev, depth, &child_path, hooks)? {
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
  /// Open `name` in `dir` to remove what it holds: its frame, listed, if it
  /// is a directory on `dev`; `None` once it is unlinked (no directory) or
  /// gone.
  fn descend(dir: RawFd, name: &CStr, dev: libc::dev_t, path: &Path, hooks: &Hooks) -> Result<Option<Frame>, Stop> {
      let fd = match open_dir_at(dir, name) {
          Ok(fd) => fd,
  ```

  with:

  ```rust
  /// Open `name` in `dir`, `depth` levels down (1: the named entry itself),
  /// to remove what it holds: its frame, listed, if it is a directory on
  /// `dev`; `None` once it is unlinked (no directory) or gone. Its depth is
  /// judged on the descriptor it opened, so a directory swapped in after
  /// its parent was listed counts too (the review's item 2).
  fn descend(
      dir: RawFd,
      name: &CStr,
      dev: libc::dev_t,
      depth: usize,
      path: &Path,
      hooks: &Hooks,
  ) -> Result<Option<Frame>, Stop> {
      let fd = match open_dir_at(dir, name) {
          Ok(fd) => fd,
          // The named entry is no directory any more: never unlinked here.
          Err(libc::ELOOP | libc::ENOTDIR) if depth == 1 => return Err(Stop::Swapped),
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
      };
      let st = stat_fd(fd.as_raw_fd()).map_err(Stop::Io)?;
  ```

  with:

  ```rust
      };
      if depth > MAX_DEPTH {
          return Err(Stop::TooDeep);
      }
      let st = stat_fd(fd.as_raw_fd()).map_err(Stop::Io)?;
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
  /// Remove the entry `name` in `dir`, which the forget names exactly (B9):
  /// a symlink is left and reported; a directory goes with its contents; any
  /// other entry is unlinked.
  pub fn remove_entry(dir: RawFd, name: &CStr, dev: libc::dev_t, path: &Path, hooks: &Hooks) -> Removal {
      match stat_at(dir, name) {
          Ok(None) => Removal::Absent,
          Ok(Some(st)) if is_link(&st) => Removal::Symlink,
          Ok(Some(st)) if is_dir(&st) => match remove_tree(dir, name, dev, path, hooks) {
              Ok(()) => Removal::Removed,
              Err(stop) => Removal::Stopped(stop),
          },
  ```

  with:

  ```rust
  /// Remove the entry `name` in `dir`, which the forget names exactly (B9),
  /// before `until`: a symlink is left and reported, one swapped in for a
  /// directory too; a directory goes with its contents; any other entry is
  /// unlinked.
  pub fn remove_entry(
      dir: RawFd,
      name: &CStr,
      dev: libc::dev_t,
      path: &Path,
      hooks: &Hooks,
      until: std::time::Instant,
  ) -> Removal {
      match stat_at(dir, name) {
          Ok(None) => Removal::Absent,
          Ok(Some(st)) if is_link(&st) => Removal::Symlink,
          Ok(Some(st)) if is_dir(&st) => {
              hooks.stated(path);
              match remove_tree(dir, name, dev, path, hooks, until) {
                  Ok(()) => Removal::Removed,
                  Err(Stop::Swapped) => match stat_at(dir, name) {
                      Ok(Some(st)) if is_link(&st) => Removal::Symlink,
                      Ok(None) => Removal::Absent,
                      _ => Removal::Stopped(Stop::Io(libc::EAGAIN)),
                  },
                  Err(stop) => Removal::Stopped(stop),
              }
          }
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
          Err(e) => Removal::Stopped(Stop::Io(e)),
      }
  }
  ```

  with:

  ```rust
          Err(e) => Removal::Stopped(Stop::Io(e)),
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn tree() -> (tempfile::TempDir, OwnedFd, libc::dev_t) {
          let dir = tempfile::tempdir().unwrap();
          std::fs::create_dir_all(dir.path().join("t/a/b")).unwrap();
          std::fs::write(dir.path().join("t/a/b/f"), "x").unwrap();
          let fd = open_root(dir.path()).unwrap();
          let dev = stat_fd(fd.as_raw_fd()).unwrap().st_dev;
          (dir, fd, dev)
      }

      fn later() -> std::time::Instant {
          std::time::Instant::now() + std::time::Duration::from_secs(60)
      }

      /// R2, the review's item 7: a directory on another device stops the
      /// removal, and nothing of it goes.
      #[test]
      fn another_file_system_stops_the_removal() {
          let (dir, fd, dev) = tree();
          let t = c_name(b"t");
          assert_eq!(
              remove_tree(fd.as_raw_fd(), &t, dev ^ 1, Path::new("t"), &Hooks::default(), later()),
              Err(Stop::MountPoint)
          );
          assert!(dir.path().join("t/a/b/f").exists());
          assert_eq!(
              remove_tree(fd.as_raw_fd(), &t, dev, Path::new("t"), &Hooks::default(), later()),
              Ok(())
          );
          assert!(!dir.path().join("t").exists());
      }

      /// B6, the review's item 3: past its deadline, a removal stops.
      #[test]
      fn a_removal_past_its_deadline_stops() {
          let (dir, fd, dev) = tree();
          let past = std::time::Instant::now();
          assert_eq!(
              remove_tree(
                  fd.as_raw_fd(),
                  &c_name(b"t"),
                  dev,
                  Path::new("t"),
                  &Hooks::default(),
                  past
              ),
              Err(Stop::Deadline)
          );
          assert!(dir.path().join("t/a/b/f").exists());
      }
  }
  ```


- [ ] **Step 4: Run them to see them pass**

- [ ] **Step 5: Revert-probes**

Each was probed and caught:
- the listing-failure arm, in both passes;
- the depth check on the opened descriptor;
- the `in_progress` refusal;
- the deadline: each of its two checks covers the other, and removing both is caught;
- the symlinked-directory report;
- every branch of `safe_mode`, among them a group named as the user that is not their primary;
- the swapped top entry;
- the mount stop, through `dev ^ 1`.

Defence in depth, not probed: the root's identity check, which no test can swap between the stat and the open.

- [ ] **Step 6: The full checks**

Expected: all pass; **1378 tests**.

- [ ] **Step 7: Commit**

```bash
git -c commit.gpgsign=false commit -am "fix(host): harden the forget walk after its security review"
```

## After this plan

**What the frontend must do (plan 4):**
- **Delete and purge results:** show `host_transcript.state`, what remains and why, and the notes, always including the context-clear limitation.
- **Pending removals:** show a list in Settings (`GET /api/settings/host-removals`), with "dismiss" behind step-up.

**Obligations this plan hands on:**
- **9d-ii (Codex):**
  - `codex app-server` with `thread/delete` against the recorded `CODEX_HOME` only;
  - the call shape pinned per Codex version in the manifest, with a contract test against a fake app-server;
  - the fallback: archive, then the rollout files by an anchored regex, run only on a spawn, initialize or unpinned failure, and reporting `codex_database_copies`;
  - the pin-bump checklist item;
  - the `codex_database_copies` per-outcome test.
- **Plan 8:** record a composed home as the root, and the real `~/.codex` too for its linked `sessions/`.
- **Recorded:**
  - **Two sessions sharing an agent session in one purge:** the first is counted partial (`shared`), and the transcript is still forgotten once.
  - **Pairs past the cap of 8:** never forgotten, and logged.
  - **The shared check:** runs in Rust over the host's and agent's kept sessions, since the audit refuses `json_each`.

**Not tested here:**
- A real Claude CLI's files; the fake adapter mirrors the SDK's `deleteSession`.
- An LDAP or sssd user database.
- A root swapped between its stat and its open.

**Spec amendments** (written back by this plan's docs commit):
- ACP core §3.3: the frames and the capability;
- ACP core §4.10: the transcript on the host, the context-clear limitation and what remains;
- ACP core §8: `agent_home`, `host_forgets`, migration 14;
- ACP core §9: `DeleteResult`, the host-removal routes and `PurgeResult.host_transcripts`;
- umbrella §6.10.

Then:
- **(9d-ii) Codex**

---

_Generated with Claude AI — please review before distribution._
