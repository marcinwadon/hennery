# Delete and purge (plan 9d-ii): the agent's own transcript on its host, Codex Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** A deleted or purged Codex session's transcript is removed on its host, completing plan 9d. Plan [9d-i](2026-10-19-host-transcripts.md) already records the home, keeps the record and carries the protocol; for Codex it answered `unsupported_agent` until now.
- **The primary path:**
  - The host runs the bundled `codex app-server` against the session's **recorded** `CODEX_HOME`, and no other.
  - It checks that the `codexHome` reported by `initialize` is that root.
  - It calls `thread/delete`, then checks afterwards that no rollout file is left.
- **The fallback:**
  - It runs only when the app-server path is unavailable: the spawn failed, `initialize` failed, the method was not found, or the version is not pinned. It also runs after three app-server timeouts in a row (B5's bounded hybrid).
  - codex-acp archives the session, then the host removes the rollout files through 9d-i's descriptor walk.
  - The result reports `codex_database_copies`: conversation copies may remain in Codex's own database.
- **The pin:** Codex's call shape is pinned per version in `adapters/manifest.json`. `hennery-pins` checks it, and the operator's pin-bump checklist re-reads it live.

**Decided by:** the operator delegated, and the parent decided on 2026-10-02:
- Codex uses option (a), with (b) as the fallback.
- The app-server never runs against another home.
- A contract test runs against a fake app-server.
- The checklist gets an item for this.

**Architecture:**
- **`hennery-host/src/forget_codex.rs`:**
  - **Before spawning:** checks the root, `sessions/` and `archived_sessions/` (B3).
  - **The app-server:**
    - `--version`, then `app-server`, both through `Adapter::spawn`'s hygiene, with the recorded `CODEX_HOME`;
    - `CODEX_SQLITE_HOME` is set to the recorded value, or removed;
    - the cwd is the root;
    - one deadline covers it all, and the whole process group is killed at the end.
  - **The calls:** `initialize`, then the `codexHome` check (`home_mismatch` is final), then `thread/delete`.
  - **The refusals:** a live worker gives `in_progress`; forked history gives `forked_history`; an ephemeral thread gives `ephemeral`. Nothing falls back after `thread/delete` has answered.
  - **The fallback:**
    - the adapter's archive;
    - B3 is checked again, and the root must be the same device and inode;
    - the walk: `sessions/` at most three date levels deep, `archived_sessions/` at its top level, an anchored rollout name, regular files only, never through a link.
- **The manifest and `hennery-pins`:** `codex_app_server` gives the Codex version, the launcher in `@openai/codex`, the `initialize` params and `thread/delete {threadId}`.
- **The collector:**
  - migration 15, `host_forgets.app_server_timeouts`;
  - three consecutive `app_server_timed_out` results set `fallback: true` on the next `forget_session`;
  - the Codex notes, among them `history.jsonl` and `logs_2.sqlite`, plus the fallback's own notes.
- **The testkit:** `hennery-fake-codex`, an app-server that checks the exact frames and answers as Codex 0.155.1 does.

**Tech Stack:** Rust (edition 2024, MSRV 1.88). No new crates.

**Spec:** ACP core §4.10 (the transcript on the host) and the parent's 9d decision, as in 9d-i. The evidence was checked against codex-acp 1.13.0 and Codex 0.155.1 (`thread/delete`, `thread/archive`, `recorder.rs`), from source and from one live run against a scratch home. Every anchor was taken from `main` at `b6d146e4`.

**Status:** executed 2026-10-02 (see "Execution status"). The security review of the code (opus, on the maintainer's behalf) answered "approve after amendments", and its scoped re-confirmation answered "confirmed with notes".

**How the code blocks were made and checked:**
- Every block below was generated from the reviewed commits, as diffs from `b6d146e4`.
- The plan was replayed from its own text onto `b6d146e4`, task by task, and the tree matched each task's commit byte for byte (`replay.py`, every block applied).

## Execution status (2026-10-02)

**Executed** on `main` at `b6d146e4`:
- an opus implementer, then the security review;
- a fresh implementer for the fixes, taking over a killed agent's unverified work in progress, and the re-confirmation;
- a scratch draft PR that ran ubuntu early, as the fleet's rule asks.

| Area | As built | Why |
|---|---|---|
| Review, binding | After the fallback's archive, the directories are opened again, B3 is checked again, the root must be the same device and inode, and the reopened ones are walked. | `archived_sessions/` usually does not exist before a home's first archive. codex-acp created it and moved the rollout there, and the record closed with the transcript still on disk. |
| B5, ruled on the maintainer's behalf | A timeout before `thread/delete` was written gives `app_server_timed_out`, retryable. Three in a row, with any other result resetting the count, set `fallback: true`. With the flag, the host spawns no Codex and runs B3, the archive and the walk. A timeout after the write stays `timed_out` and never falls back. | Endless retries would have left a slow Codex's rollouts for good. Falling back on one timeout would have let a single slow first run (Gatekeeper) settle the session. |
| Review, optional, all taken | `-32600` counts as "unavailable" only for the three texts Codex 0.155.1 gives. The launcher must be `@openai/codex`'s. The checklist re-reads the refusal texts, `codexHome` and the `--version` format. One deadline covers the walk and the adapter. A group-kill test reaches a grandchild. | Defence in depth. |
| The WIP's errors, caught by the second implementer | A flagged fallback had skipped B3's refusal. The generated files were stale. A retry re-sent an old count. A walk cut by the deadline claimed everything removed. The group-kill test could not fail. | Each fixed and probed. |
| Readings | Codex's `initialize` has no protocol version, so the pinned Codex version stands for it. The version gate applies to the bundled binary too. "Thread not found" with rollouts still present retries forever, and is visible in the host-removals list. Rollouts are reported as `transcript`. | Recorded in the spec. |

Checks:
- The five checks passed (1475 tests on `b6d146e4`, from 1437).
- 30 revert-probes were run and caught, plus the 91 of the first round.
- The forget binaries passed with 4 copies in parallel and under `umask 002`.
- The run was macOS. Ubuntu ran on the scratch PR (green on every job).

## Scope

**4 tasks:**
1. The Codex path, against a fake app-server.
2. Its hardening after the review: the error texts, the pin, the checklist, one deadline, the group kill.
3. Reopening the directories after the archive.
4. The fallback after three timeouts.

**Out of scope:**
- **Plan 8.** A composed home links `sessions/` to the user's `~/.codex`, so a forget there reports `symlink` and spawns nothing. Plan 8 must record the real root too.

## Decisions this plan makes where the spec is silent

1. **Only the recorded home** (the parent's rule).
   - `CODEX_HOME` is the registered root, and `initialize`'s `codexHome` must equal it (`home_mismatch`, final, no fallback).
   - Without a recorded home, nothing is spawned (`no_recorded_home`, final).
2. **When the fallback runs** (B5).
   - It runs on: the spawn or `initialize` failing, the method not found, an unpinned version, or three consecutive `app_server_timed_out` results.
   - It never runs after `thread/delete` answered.
   - Its result reports `codex_database_copies`, final, as `fallback_only`.
3. **What the walk may remove** (B5).
   - Only rollouts named `rollout-<timestamp>-<id>` with `.jsonl`, `.jsonl.zst`, or `_<uuid>` and then one of those, after the id has been checked.
   - Under `sessions/`, at most three date levels; under `archived_sessions/`, only its top level.
   - Only regular files, never through a link.
4. **The pin.** `codex_app_server` in the compiled-in manifest: the version, the launcher under `node_modules/@openai/codex/`, the `initialize` params, and the method fixed to `thread/delete` with one `{thread_id}` key. `hennery-pins` ties it to the lockfile's `@openai/codex` at that version. `--use-cli` runs only a version pinned there.
5. **Who may write.** The same rule as 9d-i: the owner, or the user's private group.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`.
- The five checks pass.
- No new crates. **Wire types change:** `fallback` on `forget_session`, and the new reasons. Regenerate them.
- Every collector query names the owner.
- No production test hooks. Nothing is macOS-only.
- Every commit is authored by the gmail identity.

## Review Focus

1. **The recorded home, and nothing else.**
   - Tests: the `home_mismatch`, `no_recorded_home` and symlinked-kind-directory tests in `forget_codex.rs`.
2. **The fallback never removes too much, and never too little.**
   - Tests: the rollout-name unit tests, the depth tests, and "a Codex fallback into a new archived_sessions leaves nothing".
3. **Never fall back after `thread/delete` answered; fall back after three timeouts.**
   - Tests: the refusal mappings, and "a slow Codex app-server falls back after three timeouts in a row".
4. **One deadline, and the whole group killed.**
   - Tests: "a forget's app-server dies with its whole process group", and "a fallback adapter that never answers is stopped within the deadline".

**Reading the steps:** as in plan 9a.

---

### Task 1: The Codex path, against a fake app-server

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-host/tests/runtime.rs`, replace:

  ```rust
      assert_eq!(prepared.agents.profiles["codex"], Profile::Generic);
  }
  ```

  with:

  ```rust
      assert_eq!(prepared.agents.profiles["codex"], Profile::Generic);
      // Plan 9d decision 9: a forget's app-server is the set's bundled Codex,
      // `<node> <set>/codex/<the pinned launcher>`, once the set has it.
      assert!(
          prepared.agents.codex_app_server.is_none(),
          "this set bundles no launcher"
      );
      let pin = hennery_host::runtime::manifest::Manifest::embedded()
          .codex_app_server
          .unwrap();
      let launcher = set.path.join("codex").join(&pin.bin);
      std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
      std::fs::write(&launcher, "// codex.js").unwrap();
      let app_server = agents::from_set(&set, &std::collections::BTreeMap::new())
          .codex_app_server
          .expect("the bundled app-server");
      assert_eq!(app_server.program, set.node.to_string_lossy());
      assert_eq!(app_server.args, [launcher.to_string_lossy().into_owned()]);
      assert!(app_server.env.is_empty());
  }
  ```

  In `crates/hennery-host/tests/runtime.rs`, replace:

  ```rust
      );
      // The override gone (a hand edit, say): claude is unavailable.
  ```

  with:

  ```rust
      );
      // Plan 9d decision 9: `--use-cli codex=…` is the app-server too.
      std::fs::write(
          dir.join("host.toml"),
          "[cli]\nclaude = \"/bin/sh\"\ncodex = \"/bin/cat\"\n",
      )
      .unwrap();
      let prepared = agents::prepare(&dir, Some(&selection), Some(&server.sources()), &quiet).await;
      let app_server = prepared.agents.codex_app_server.expect("the operator's codex");
      assert_eq!((app_server.program.as_str(), app_server.args.len()), ("/bin/cat", 0));
      std::fs::write(dir.join("host.toml"), "[cli]\nclaude = \"/bin/sh\"\n").unwrap();
      // The override gone (a hand edit, say): claude is unavailable.
  ```

  In `crates/hennery-host/tests/support/mod.rs`, replace:

  ```rust
                  adapters: BTreeMap::new(),
              },
  ```

  with:

  ```rust
                  adapters: BTreeMap::new(),
                  codex_app_server: None,
              },
  ```

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust
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
  ```

  with:

  ```rust
  /// A Codex rollout of `AGENT_SESSION` under `root`, as Codex names it, and
  /// another thread's beside it.
  fn codex_rollouts(root: &Path) -> (PathBuf, PathBuf) {
      let day = root.join("sessions/2026/10/02");
      std::fs::create_dir_all(&day).unwrap();
      let own = day.join(format!("rollout-2026-10-02T10-00-00-{AGENT_SESSION}.jsonl"));
      let other = day.join("rollout-2026-10-02T10-00-00-1b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3.jsonl");
      std::fs::write(&own, "rollout").unwrap();
      std::fs::write(&other, "keep").unwrap();
      (own, other)
  }

  /// The fake app-server (Codex 0.155.1), logging to `log`.
  fn fake_codex(log: &Path) -> AgentCommand {
      let script = hennery_testkit::FakeCodex {
          version: "0.155.1".into(),
          log: log.to_str().unwrap().into(),
          ..hennery_testkit::FakeCodex::default()
      };
      let mut command = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-codex")).unwrap();
      command.env.push((
          hennery_testkit::CODEX_SCRIPT_ENV.into(),
          serde_json::to_string(&script).unwrap(),
      ));
      command
  }

  /// Decision 9 end to end: a Codex session deleted over HTTP has its thread
  /// deleted by the bundled app-server under its recorded home: `removed`,
  /// the record gone, Codex's other residue named in the notes (O12), and no
  /// root in the answer (B2).
  #[tokio::test]
  async fn a_delete_over_http_deletes_the_codex_thread_through_the_app_server() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      let log = roots.base.join("codex.log");
      let mut cfg = host_config(collector.addr, &roots, &answering(AGENT_SESSION));
      cfg.codex_app_server = Some(fake_codex(&log));
      start_host(cfg);
      connected(&collector, true).await;
      let session = start_session(&collector, "codex", &roots).await;
      let (own, other) = codex_rollouts(&roots.codex());
      let (result, body) = deleted(&collector, &session).await;
      let removal = result.host_transcript;
      assert_eq!(removal.state, RemovalState::Removed, "{body}");
      assert_eq!(removal.notes, hennery_sessions::forget::CODEX_NOTES, "{body}");
      assert!(!own.exists() && other.exists());
      assert!(std::fs::read_to_string(&log).unwrap().contains("thread/delete"));
      assert!(!body.contains(roots.base.to_str().unwrap()), "{body}");
      assert!(removals(&collector).await.is_empty());
  }

  /// Decision 10 end to end: with no app-server to run, the fallback
  /// archives through the adapter and removes the rollouts itself: `partial`,
  /// with `codex_database_copies` left for good, its notes with it, and the
  /// record final.
  #[tokio::test]
  async fn a_codex_delete_without_the_app_server_falls_back_and_says_what_remains() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      let archive_log = roots.base.join("archive.log");
      let script = FakeScript {
          codex_archive_log: Some(archive_log.to_str().unwrap().into()),
          ..answering(AGENT_SESSION)
      };
      start_host(host_config(collector.addr, &roots, &script));
      connected(&collector, true).await;
      let session = start_session(&collector, "codex", &roots).await;
      let (own, other) = codex_rollouts(&roots.codex());
  ```

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust
          [(ForgetKind::Session, ForgetReason::UnsupportedAgent)]
      );
      assert!(removal.notes.is_empty());
      assert!(!body.contains(roots.base.to_str().unwrap()), "{body}");
  ```

  with:

  ```rust
          [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly)],
          "{body}"
      );
      for note in hennery_sessions::forget::CODEX_FALLBACK_NOTES {
          assert!(removal.notes.iter().any(|n| n == note), "{note}: {body}");
      }
      assert!(!own.exists() && other.exists());
      assert!(std::fs::read_to_string(&archive_log).unwrap().contains("CODEX_HOME="));
  ```

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust
      // The delete's own attempt; the close's `session_closed` may ask for
      // one more (the review's item 11). One or two, never a runaway loop.
      assert_eq!(
          (item.session_id.as_str(), item.agent.as_str(), item.state),
          (session.as_str(), "codex", HostRemovalState::Pending)
      );
      assert!((1..=2).contains(&item.attempts), "{item:?}");
  ```

  with:

  ```rust
      assert_eq!((item.agent.as_str(), item.state), ("codex", HostRemovalState::Final));
      // The list says what the fallback leaves too.
      let listed_notes = &item.last_result.as_ref().expect("a result").notes;
      for note in hennery_sessions::forget::CODEX_FALLBACK_NOTES {
          assert!(listed_notes.iter().any(|n| n == note), "{note}: {listed_notes:?}");
      }
  }

  /// The parent's rule, B1: a Codex forget for a home the host never
  /// registered spawns no app-server and no adapter; and a Codex session
  /// with no recorded home (its `CODEX_HOME` did not resolve at the start)
  /// is final, `no_recorded_home`, with nothing sent to the host at all.
  #[tokio::test]
  async fn a_codex_home_mismatched_or_absent_spawns_nothing() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      let log = roots.base.join("codex.log");
      let archive_log = roots.base.join("archive.log");
      let script = FakeScript {
          codex_archive_log: Some(archive_log.to_str().unwrap().into()),
          ..answering(AGENT_SESSION)
      };
      let mut cfg = host_config(collector.addr, &roots, &script);
      cfg.codex_app_server = Some(fake_codex(&log));
      start_host(cfg);
      connected(&collector, true).await;
      // Mismatched: the stored home rewritten to another root.
      let session = start_session(&collector, "codex", &roots).await;
      let elsewhere = roots.base.join("elsewhere");
      let (theirs, _) = codex_rollouts(&elsewhere);
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
      assert!(theirs.exists());
      // Absent: no recorded home at all.
      let session = start_session(&collector, "codex", &roots).await;
      rusqlite::Connection::open(&db)
          .unwrap()
          .execute("UPDATE sessions SET agent_home = NULL WHERE id = ?1", [session.clone()])
          .unwrap();
      let (result, body) = deleted(&collector, &session).await;
      assert_eq!(
          result
              .host_transcript
              .remaining
              .iter()
              .map(|r| r.reason)
              .collect::<Vec<_>>(),
          [ForgetReason::NoRecordedHome],
          "{body}"
      );
      assert!(!log.exists(), "an app-server was spawned");
      assert!(!archive_log.exists(), "an adapter's delete ran");
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
              account: None,
          }
  ```

  with:

  ```rust
              account: None,
              codex_app_server: None,
              codex_pin: None,
              deadline: hennery_host::forget::FORGET_DEADLINE,
          }
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
  /// Decision 8: other agents are not forgotten yet (9d-ii).
  #[tokio::test]
  async fn a_codex_forget_is_unsupported_for_now() {
      let root = Root::new();
      let mut codex = root.forget_at(&root.root());
      codex.agent = "codex".into();
      let forgotten = forget(&root.ctx(Hooks::default()), &codex).await;
  ```

  with:

  ```rust
  /// Decision 8, 9d-ii: an agent this host cannot forget for (neither
  /// Claude nor Codex) is answered `unsupported_agent`, retryable, and
  /// nothing is touched.
  #[tokio::test]
  async fn an_agent_with_no_forget_is_unsupported() {
      let root = Root::new();
      root.populate();
      let before = root.tree();
      let mut other = root.forget_at(&root.root());
      other.agent = "gemini".into();
      let forgotten = forget(&root.ctx(Hooks::default()), &other).await;
      assert_eq!(root.tree(), before);
      assert_eq!(root.adapter_ran(), None);
  ```

  Create `crates/hennery-testkit/tests/forget_codex.rs`:

  ```rust
  //! The Codex path of a forget on the host (plan 9d decisions 9, 10, 12;
  //! B4–B6), run directly against roots of the test's own. The fake
  //! app-server (`hennery-fake-codex`) stands in for Codex 0.155.1's CLI and
  //! the fake adapter for codex-acp: the contract test checks the exact frames
  //! the host sends, and every answer maps to its outcome. These run on Linux
  //! CI as on macOS (R4).

  use hennery_host::AgentCommand;
  use hennery_host::forget::{FORGET_DEADLINE, Forget, ForgetContext, Forgotten, forget};
  use hennery_host::runtime::manifest::Manifest;
  use hennery_host::walk::Hooks;
  use hennery_proto::frames::{AgentHome, ForgetKind, ForgetReason};
  use hennery_testkit::{CODEX_SCRIPT_ENV, FakeCodex, FakeDelete, FakeInitialize, FakeScript, SCRIPT_ENV};
  use std::collections::HashMap;
  use std::os::unix::fs::symlink;
  use std::path::{Path, PathBuf};
  use std::time::{Duration, Instant};

  /// A thread id as Codex makes them (a UUID v7, lowercase).
  const ID: &str = "019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b";
  const STAMP: &str = "2026-10-02T10-00-00";

  /// The session's rollouts Codex may have written: plain, a reverted
  /// thread's (`_<rollout id>`), compressed, and one archived already.
  const OWN: [&str; 3] = [
      "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      "sessions/2026/10/01/rollout-2026-10-01T09-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b_019a0b1c-2d3e-7f40-8a5b-111111111111.jsonl.zst",
      "archived_sessions/rollout-2026-09-30T08-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
  ];

  /// Entries beside them that are not the session's, or out of the walk's
  /// reach, and stay.
  const NEIGHBOURS: [&str; 12] = [
      // Another thread.
      "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-000000000000.jsonl",
      // Another thread, whose reverted rollout id is this session's id.
      "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-000000000000_019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      // The id with a suffix, a prefix, or another extension.
      "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1bf.jsonl",
      "sessions/2026/10/02/xrollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      "sessions/2026/10/02/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl.bak",
      // Past `sessions/`'s three levels (a fourth named like a day too), in
      // a directory that is no date, and below `archived_sessions/`'s top
      // (in one named like a year too).
      "sessions/2026/10/02/deeper/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      "sessions/2026/10/02/03/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      "sessions/backup/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      "archived_sessions/old/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      "archived_sessions/2026/rollout-2026-10-02T10-00-00-019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b.jsonl",
      // Codex's other files: never the host's to touch (O12).
      "history.jsonl",
      "session_index.jsonl",
  ];

  /// The umask these tests assume (B3), as `forget_claude.rs` sets it.
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
          for sub in ["codex", "host", "home", "outside", "sqlite"] {
              std::fs::create_dir(base.join(sub)).unwrap();
          }
          Self { _dir: dir, base }
      }

      fn root(&self) -> PathBuf {
          self.base.join("codex")
      }

      fn at(&self, rel: &str) -> PathBuf {
          self.root().join(rel)
      }

      fn outside(&self) -> PathBuf {
          self.base.join("outside")
      }

      fn codex_log(&self) -> PathBuf {
          self.base.join("codex.log")
      }

      fn archive_log(&self) -> PathBuf {
          self.base.join("archive.log")
      }

      /// The fake app-server answering as `delete` says, version 0.155.1.
      fn fake(&self, delete: FakeDelete) -> FakeCodex {
          FakeCodex {
              version: "0.155.1".into(),
              log: self.codex_log().to_str().unwrap().into(),
              delete,
              ..FakeCodex::default()
          }
      }

      /// The forget's context: `codex` as the fake app-server, and as the
      /// fake adapter in codex-acp's archive mode. Each also carries
      /// variables of its own that the forget must override or strip (B6).
      fn ctx(&self, codex: FakeCodex) -> ForgetContext {
          let mut app_server = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-codex")).unwrap();
          app_server
              .env
              .push((CODEX_SCRIPT_ENV.into(), serde_json::to_string(&codex).unwrap()));
          let script = FakeScript {
              codex_archive_log: Some(self.archive_log().to_str().unwrap().into()),
              ..FakeScript::default()
          };
          let mut adapter = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
          adapter
              .env
              .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
          let outside = self.outside().to_str().unwrap().to_string();
          for command in [&mut app_server, &mut adapter] {
              command.env.push(("CODEX_HOME".into(), outside.clone()));
              command.env.push(("CODEX_SQLITE_HOME".into(), outside.clone()));
              command
                  .env
                  .push(("CLAUDE_CODE_PROJECT_DIR_NAME".into(), "elsewhere".into()));
          }
          ForgetContext {
              agents: HashMap::from([("codex".to_string(), adapter)]),
              data_dir: self.base.join("host"),
              home: Some(self.base.join("home")),
              hooks: Hooks::default(),
              account: None,
              codex_app_server: Some(app_server),
              codex_pin: Manifest::embedded().codex_app_server,
              deadline: FORGET_DEADLINE,
          }
      }

      fn forget_at(&self, root: &Path, sqlite_root: Option<&Path>) -> Forget {
          Forget {
              agent: "codex".into(),
              agent_session_id: ID.into(),
              agent_home: AgentHome {
                  root: root.to_str().unwrap().into(),
                  sqlite_root: sqlite_root.map(|p| p.to_str().unwrap().into()),
              },
          }
      }

      async fn forget_with(&self, ctx: &ForgetContext) -> Forgotten {
          forget(ctx, &self.forget_at(&self.root(), None)).await
      }

      /// The session's rollouts and the neighbours that stay.
      fn populate(&self) {
          for rel in OWN.iter().chain(NEIGHBOURS.iter()) {
              let path = self.at(rel);
              std::fs::create_dir_all(path.parent().unwrap()).unwrap();
              std::fs::write(path, "rollout").unwrap();
          }
      }

      fn exists(&self, rel: &str) -> bool {
          std::fs::symlink_metadata(self.at(rel)).is_ok()
      }

      fn own_left(&self) -> usize {
          OWN.iter().filter(|rel| self.exists(rel)).count()
      }

      fn neighbours_kept(&self) {
          for rel in NEIGHBOURS {
              assert!(self.exists(rel), "{rel} was removed");
          }
      }

      /// The fake app-server's log: its spawns, and the lines it read.
      fn codex_lines(&self) -> Vec<serde_json::Value> {
          std::fs::read_to_string(self.codex_log())
              .unwrap_or_default()
              .lines()
              .map(|l| serde_json::from_str(l).unwrap())
              .collect()
      }

      fn spawns(&self) -> Vec<serde_json::Value> {
          self.codex_lines()
              .into_iter()
              .filter_map(|l| l.get("spawn").cloned())
              .collect()
      }

      fn received(&self) -> Vec<String> {
          self.codex_lines()
              .into_iter()
              .filter_map(|l| l.get("recv").and_then(|r| r.as_str().map(str::to_string)))
              .collect()
      }

      fn archived(&self) -> Option<String> {
          std::fs::read_to_string(self.archive_log()).ok()
      }
  }

  fn reasons(forgotten: &Forgotten) -> Vec<(ForgetKind, ForgetReason, bool)> {
      forgotten
          .remaining
          .iter()
          .map(|r| (r.what.kind, r.reason, r.retry))
          .collect()
  }

  fn count_left(forgotten: &Forgotten, kind: ForgetKind) -> u32 {
      forgotten
          .remaining
          .iter()
          .filter(|r| r.what.kind == kind)
          .map(|r| r.what.count)
          .sum()
  }

  fn removed(forgotten: &Forgotten, kind: ForgetKind) -> u32 {
      forgotten
          .removed
          .iter()
          .filter(|w| w.kind == kind)
          .map(|w| w.count)
          .sum()
  }

  /// The exact frames the host sends Codex 0.155.1 (decision 9): the pinned
  /// `initialize`, then `thread/delete` with the thread id. One JSON object
  /// per line, no `jsonrpc` field, as codex-acp sends them.
  fn pinned_frames() -> [String; 2] {
      [
          r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"hennery","title":"hennery","version":"1"}}}"#
              .to_string(),
          format!(r#"{{"id":2,"method":"thread/delete","params":{{"threadId":"{ID}"}}}}"#),
      ]
  }

  /// The spawn record the fake wrote for `args`, as the forget ran it (B6):
  /// `CODEX_HOME` the recorded root whatever the command's own said, the
  /// project-name override stripped, the root its cwd.
  fn assert_spawned(spawn: &serde_json::Value, args: &[&str], root: &Path, sqlite: &str) {
      let root = root.to_str().unwrap();
      assert_eq!(spawn["args"], serde_json::json!(args), "{spawn}");
      assert_eq!(spawn["CODEX_HOME"], root, "{spawn}");
      assert_eq!(spawn["CODEX_SQLITE_HOME"], sqlite, "{spawn}");
      assert_eq!(spawn["CLAUDE_CODE_PROJECT_DIR_NAME"], "-", "{spawn}");
      assert_eq!(spawn["cwd"], root, "{spawn}");
  }

  /// The contract test (decision 9): the version is asked first, then the
  /// app-server gets exactly the pinned frames; Codex's answer and its
  /// unsolicited notifications are read past; the rollouts are gone, the
  /// check afterwards finds none (B4), and nothing else is touched. No
  /// fallback runs.
  #[tokio::test]
  async fn the_app_server_gets_exactly_the_pinned_frames_and_the_thread_goes() {
      let root = Root::new();
      root.populate();
      let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Delete))).await;
      assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
      assert_eq!(removed(&forgotten, ForgetKind::Transcript), OWN.len() as u32);
      assert_eq!(root.own_left(), 0);
      root.neighbours_kept();
      let spawns = root.spawns();
      assert_eq!(spawns.len(), 2, "{spawns:?}");
      assert_spawned(&spawns[0], &["--version"], &root.root(), "-");
      assert_spawned(&spawns[1], &["app-server"], &root.root(), "-");
      assert_eq!(root.received(), pinned_frames());
      assert_eq!(root.archived(), None, "no fallback");
      // A second forget finds nothing: Codex's "no rollout found" counts for
      // nothing, the check afterwards decides.
      let again = root.forget_with(&root.ctx(root.fake(FakeDelete::Delete))).await;
      assert_eq!(reasons(&again), [], "{again:?}");
      assert_eq!(removed(&again, ForgetKind::Transcript), 0);
  }

  /// B6: a recorded `CODEX_SQLITE_HOME` is the one the app-server gets; with
  /// none recorded, the command's own is stripped (above: `-`).
  #[tokio::test]
  async fn a_recorded_sqlite_home_is_the_app_servers() {
      let root = Root::new();
      root.populate();
      let sqlite = root.base.join("sqlite");
      let forgotten = forget(
          &root.ctx(root.fake(FakeDelete::Delete)),
          &root.forget_at(&root.root(), Some(&sqlite)),
      )
      .await;
      assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
      let spawns = root.spawns();
      assert_eq!(spawns.len(), 2, "{spawns:?}");
      for (spawn, args) in spawns.iter().zip([["--version"], ["app-server"]]) {
          assert_spawned(spawn, &args, &root.root(), sqlite.to_str().unwrap());
      }
  }

  /// Decision 9, B5: each of `thread/delete`'s refusals maps to its own
  /// reason, retryable only for a live worker; nothing is removed, and the
  /// fallback never runs after one.
  #[tokio::test]
  async fn each_refusal_of_thread_delete_maps_to_its_reason_and_never_falls_back() {
      for (delete, reason, retry) in [
          (FakeDelete::ForkedHistory, ForgetReason::ForkedHistory, false),
          (FakeDelete::Ephemeral, ForgetReason::Ephemeral, false),
          (FakeDelete::LiveWorker, ForgetReason::InProgress, true),
      ] {
          let root = Root::new();
          root.populate();
          let forgotten = root.forget_with(&root.ctx(root.fake(delete))).await;
          assert_eq!(
              reasons(&forgotten),
              [
                  (ForgetKind::Session, reason, retry),
                  (ForgetKind::Transcript, reason, retry)
              ],
              "{delete:?}: {forgotten:?}"
          );
          assert_eq!(
              count_left(&forgotten, ForgetKind::Transcript),
              OWN.len() as u32,
              "{delete:?}"
          );
          assert_eq!(root.own_left(), OWN.len(), "{delete:?}");
          assert_eq!(root.received(), pinned_frames(), "{delete:?}");
          assert_eq!(root.archived(), None, "{delete:?}: no fallback after a refusal");
      }
  }

  /// B5: an error `thread/delete` answered that is none of its known
  /// refusals (a failure midway) is retried, and never followed by the
  /// fallback either.
  #[tokio::test]
  async fn an_unknown_failure_of_thread_delete_is_retried_and_never_falls_back() {
      let root = Root::new();
      root.populate();
      let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Internal))).await;
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Session, ForgetReason::IoError, true),
              (ForgetKind::Transcript, ForgetReason::IoError, true)
          ],
          "{forgotten:?}"
      );
      assert_eq!(root.archived(), None);
  }

  /// B5: an app-server that ends once `thread/delete` was sent, answering
  /// nothing, is retried and never followed by the fallback: the delete may
  /// have run.
  #[tokio::test]
  async fn an_app_server_that_ends_after_the_delete_was_sent_never_falls_back() {
      let root = Root::new();
      root.populate();
      let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Exit))).await;
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Session, ForgetReason::IoError, true),
              (ForgetKind::Transcript, ForgetReason::IoError, true)
          ],
          "{forgotten:?}"
      );
      assert_eq!(root.received(), pinned_frames());
      assert_eq!(root.archived(), None);
  }

  /// B4: an app-server that answers success but leaves a rollout makes the
  /// result partial, retryable: the check afterwards decides, not the answer.
  #[tokio::test]
  async fn a_rollout_left_behind_makes_the_result_partial() {
      let root = Root::new();
      root.populate();
      let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::AnswerButKeep))).await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Transcript, ForgetReason::StillPresent, true)],
          "{forgotten:?}"
      );
      assert_eq!(count_left(&forgotten, ForgetKind::Transcript), OWN.len() as u32);
      assert_eq!(root.archived(), None);
  }

  /// The parent's rule: an app-server that resolved another `CODEX_HOME`
  /// than the recorded one is asked nothing more, and no fallback runs (its
  /// archive would resolve the same wrong home).
  #[tokio::test]
  async fn an_app_server_on_another_home_is_asked_nothing_and_nothing_falls_back() {
      let root = Root::new();
      root.populate();
      let mut fake = root.fake(FakeDelete::Delete);
      fake.codex_home = Some(root.outside().to_str().unwrap().into());
      let forgotten = root.forget_with(&root.ctx(fake)).await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Session, ForgetReason::HomeMismatch, false)],
          "{forgotten:?}"
      );
      assert_eq!(root.received(), pinned_frames()[..1]);
      assert_eq!(root.own_left(), OWN.len());
      assert_eq!(root.archived(), None);
  }

  /// What the fallback leaves (decision 10): the archive ran under the
  /// recorded root (B6), then the rollouts went through the descriptor walk,
  /// and Codex's own database copies are reported, final.
  fn assert_fell_back(root: &Root, forgotten: &Forgotten, why: &str) {
      assert_eq!(
          reasons(forgotten),
          [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)],
          "{why}: {forgotten:?}"
      );
      assert_eq!(removed(forgotten, ForgetKind::Transcript), OWN.len() as u32, "{why}");
      assert_eq!(root.own_left(), 0, "{why}");
      root.neighbours_kept();
      let r = root.root();
      let r = r.to_str().unwrap();
      assert_eq!(
          root.archived().as_deref(),
          Some(format!("CODEX_HOME={r}\ncwd={r}\nCODEX_SQLITE_HOME=-\n").as_str()),
          "{why}"
      );
  }

  /// B5: the fallback runs when the app-server path is unavailable, and
  /// only then: no binary, a binary that does not spawn, a version the
  /// manifest pins no shape for (or no pin at all), an `initialize` that
  /// fails or ends, and a `thread/delete` this Codex does not know (0.155.1
  /// answers an unknown method `-32600 Invalid request`, and JSON-RPC's own
  /// is `-32601`).
  #[tokio::test]
  async fn the_fallback_runs_only_when_the_app_server_is_unavailable() {
      struct Case {
          why: &'static str,
          set: fn(&Root, &mut ForgetContext),
          /// The app-server's spawns (`--version`, `app-server`) expected.
          spawns: usize,
      }
      fn fake_with(root: &Root, ctx: &mut ForgetContext, edit: fn(&mut FakeCodex)) {
          let mut fake = root.fake(FakeDelete::Delete);
          edit(&mut fake);
          *ctx = root.ctx(fake);
      }
      let cases = [
          Case {
              why: "no binary",
              set: |_, ctx| ctx.codex_app_server = None,
              spawns: 0,
          },
          Case {
              why: "a binary that does not spawn",
              set: |root, ctx| {
                  ctx.codex_app_server = Some(AgentCommand::parse(root.base.join("missing").to_str().unwrap()).unwrap())
              },
              spawns: 0,
          },
          Case {
              why: "no pinned shape",
              set: |_, ctx| ctx.codex_pin = None,
              spawns: 0,
          },
          Case {
              why: "an unpinned version",
              set: |root, ctx| fake_with(root, ctx, |f| f.version = "0.156.0".into()),
              spawns: 1,
          },
          Case {
              why: "initialize refused",
              set: |root, ctx| fake_with(root, ctx, |f| f.initialize = FakeInitialize::Error),
              spawns: 2,
          },
          Case {
              why: "initialize ends the app-server",
              set: |root, ctx| fake_with(root, ctx, |f| f.initialize = FakeInitialize::Exit),
              spawns: 2,
          },
          Case {
              why: "thread/delete unknown to this Codex",
              set: |root, ctx| fake_with(root, ctx, |f| f.delete = FakeDelete::UnknownMethod),
              spawns: 2,
          },
          Case {
              why: "method not found",
              set: |root, ctx| fake_with(root, ctx, |f| f.delete = FakeDelete::MethodNotFound),
              spawns: 2,
          },
      ];
      for case in cases {
          let root = Root::new();
          root.populate();
          let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
          (case.set)(&root, &mut ctx);
          let forgotten = root.forget_with(&ctx).await;
          assert_fell_back(&root, &forgotten, case.why);
          assert_eq!(root.spawns().len(), case.spawns, "{}: {:?}", case.why, root.spawns());
      }
  }

  /// B6: a `--version`, an `initialize` or a `thread/delete` that never answers is cut at
  /// the app-server's share of the forget's deadline, retryable; the
  /// app-server's whole group is killed, though it ignores SIGTERM; and no
  /// fallback runs: the delete may have been under way, and a slow start (a
  /// first exec macOS scans) is no reason to settle for the fallback's final
  /// result.
  #[tokio::test]
  async fn an_app_server_that_never_answers_is_cut_at_the_deadline_and_its_group_killed() {
      for (hang, frames) in [("thread/delete", 2), ("initialize", 1), ("--version", 0)] {
          let root = Root::new();
          root.populate();
          let pid_file = root.base.join("app-server.pid");
          let mut fake = root.fake(FakeDelete::Hang);
          match hang {
              "initialize" => fake.initialize = FakeInitialize::Hang,
              "--version" => fake.hang_version = true,
              _ => {}
          }
          fake.pid_file = Some(pid_file.to_str().unwrap().into());
          fake.ignore_term = true;
          let mut ctx = root.ctx(fake);
          ctx.deadline = Duration::from_secs(10);
          let started = Instant::now();
          let forgotten = tokio::time::timeout(Duration::from_secs(40), root.forget_with(&ctx))
              .await
              .expect("the forget keeps its deadline");
          assert!(
              started.elapsed() < Duration::from_secs(20),
              "{hang}: {:?}",
              started.elapsed()
          );
          assert_eq!(
              reasons(&forgotten),
              [
                  (ForgetKind::Session, ForgetReason::TimedOut, true),
                  (ForgetKind::Transcript, ForgetReason::TimedOut, true)
              ],
              "{hang}: {forgotten:?}"
          );
          assert_eq!(root.received(), pinned_frames()[..frames], "{hang}");
          assert_eq!(root.archived(), None, "{hang}: no fallback");
          assert_gone(&pid_file).await;
      }
  }

  /// The process whose pid `pid_file` holds is gone (its group killed).
  async fn assert_gone(pid_file: &Path) {
      let pid: i32 = std::fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();
      let deadline = Instant::now() + Duration::from_secs(5);
      while hennery_testkit::pid_alive(pid) && Instant::now() < deadline {
          tokio::time::sleep(Duration::from_millis(50)).await;
      }
      assert!(!hennery_testkit::pid_alive(pid), "the app-server outlived its forget");
  }

  /// B3, decision 12: `sessions/` or `archived_sessions/` that is a symlink
  /// (a composed home's linked `sessions/`, say) is reported, never
  /// followed: nothing is spawned, since `thread/delete` and the archive
  /// would both follow it, and its target is untouched.
  #[tokio::test]
  async fn a_symlinked_sessions_directory_spawns_nothing() {
      for kind_dir in ["sessions", "archived_sessions"] {
          let root = Root::new();
          let target = root.outside().join("real");
          std::fs::create_dir_all(&target).unwrap();
          let theirs = target.join(format!("rollout-{STAMP}-{ID}.jsonl"));
          std::fs::write(&theirs, "keep").unwrap();
          symlink(&target, root.at(kind_dir)).unwrap();
          let forgotten = root.forget_with(&root.ctx(root.fake(FakeDelete::Delete))).await;
          assert_eq!(
              reasons(&forgotten),
              [(ForgetKind::Transcript, ForgetReason::Symlink, false)],
              "{kind_dir}: {forgotten:?}"
          );
          assert!(theirs.exists(), "{kind_dir}: the target was touched");
          assert_eq!(root.spawns().len(), 0, "{kind_dir}");
          assert_eq!(root.archived(), None, "{kind_dir}");
      }
  }

  /// Decision 11's spirit on the host: a root that is not there (any more)
  /// spawns nothing.
  #[tokio::test]
  async fn a_missing_root_spawns_nothing() {
      let root = Root::new();
      let gone = root.base.join("gone");
      let forgotten = forget(&root.ctx(root.fake(FakeDelete::Delete)), &root.forget_at(&gone, None)).await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Session, ForgetReason::RootMissing, false)],
          "{forgotten:?}"
      );
      assert_eq!(root.spawns().len(), 0);
      assert_eq!(root.archived(), None);
  }

  /// Decision 10, B5, R3: the fallback's own walk (here with no adapter to
  /// archive first) removes the session's rollouts in `sessions/` at most
  /// three levels down and at the top of `archived_sessions/`, regular files
  /// only: a rollout-named symlink is reported and left, its target too; a
  /// symlinked date directory is never entered.
  #[tokio::test]
  async fn the_fallback_walk_removes_only_the_sessions_own_rollout_files() {
      let root = Root::new();
      root.populate();
      let linked_target = root.outside().join("rollout");
      std::fs::write(&linked_target, "keep").unwrap();
      symlink(
          &linked_target,
          root.at(&format!("archived_sessions/rollout-{STAMP}-{ID}.jsonl.zst")),
      )
      .unwrap();
      let day = root.outside().join("day");
      std::fs::create_dir_all(&day).unwrap();
      let in_linked_day = day.join(format!("rollout-{STAMP}-{ID}.jsonl"));
      std::fs::write(&in_linked_day, "keep").unwrap();
      symlink(&day, root.at("sessions/2026/10/03")).unwrap();
      let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
      ctx.codex_app_server = None;
      ctx.agents.clear();
      let forgotten = root.forget_with(&ctx).await;
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Transcript, ForgetReason::Symlink, false),
              (ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)
          ],
          "{forgotten:?}"
      );
      // The rollout-named link and the linked day, each counted.
      assert_eq!(count_left(&forgotten, ForgetKind::Transcript), 2, "{forgotten:?}");
      assert_eq!(removed(&forgotten, ForgetKind::Transcript), OWN.len() as u32);
      assert_eq!(root.own_left(), 0);
      root.neighbours_kept();
      assert!(linked_target.exists() && in_linked_day.exists(), "a link was followed");
      assert!(root.exists(&format!("archived_sessions/rollout-{STAMP}-{ID}.jsonl.zst")));
  }

  /// R3 for the fallback's walk: a date directory swapped for a symlink
  /// after it was found to be a directory, before it is opened, is never
  /// entered, and its target stays.
  #[tokio::test]
  async fn a_date_directory_swapped_for_a_symlink_is_never_entered() {
      let root = Root::new();
      root.populate();
      let target = root.outside().join("swapped");
      std::fs::create_dir_all(&target).unwrap();
      let theirs = target.join(format!("rollout-{STAMP}-{ID}.jsonl"));
      std::fs::write(&theirs, "keep").unwrap();
      let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
      ctx.codex_app_server = None;
      ctx.agents.clear();
      let day = root.at("sessions/2026/10/02");
      let (day_hook, target_hook) = (day.clone(), target.clone());
      let swapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
      let seen = swapped.clone();
      ctx.hooks.stated = Some(std::sync::Arc::new(move |path: &Path| {
          let real_dir = std::fs::symlink_metadata(&day_hook).is_ok_and(|m| m.is_dir());
          if path.ends_with("sessions/2026/10/02") && real_dir {
              std::fs::rename(&day_hook, day_hook.with_extension("moved")).unwrap();
              symlink(&target_hook, &day_hook).unwrap();
              seen.store(true, std::sync::atomic::Ordering::SeqCst);
          }
      }));
      let forgotten = root.forget_with(&ctx).await;
      assert!(swapped.load(std::sync::atomic::Ordering::SeqCst), "the hook never ran");
      assert!(theirs.exists(), "the swapped-in target was touched: {forgotten:?}");
      assert!(std::fs::symlink_metadata(&day).unwrap().file_type().is_symlink());
      assert!(
          reasons(&forgotten).contains(&(ForgetKind::Transcript, ForgetReason::Symlink, false)),
          "{forgotten:?}"
      );
  }

  /// B3 for the fallback's walk: a date directory it cannot list, and a
  /// rollout it cannot unlink (its directory read-only), are left for a
  /// retry, and counted.
  #[tokio::test]
  async fn what_the_walk_could_not_list_or_unlink_is_retried() {
      use std::os::unix::fs::PermissionsExt;
      // SAFETY: geteuid(2) cannot fail.
      if unsafe { libc::geteuid() } == 0 {
          // Root unlinks in a read-only directory: nothing to see.
          eprintln!("skipped: running as root");
          return;
      }
      let root = Root::new();
      root.populate();
      let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
      ctx.codex_app_server = None;
      ctx.agents.clear();
      ctx.hooks.fail_listing = Some(std::sync::Arc::new(|path: &Path| path.ends_with("sessions/2026/10/01")));
      let locked = root.at("sessions/2026/10/02");
      std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
      let forgotten = root.forget_with(&ctx).await;
      std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Transcript, ForgetReason::IoError, true),
              (ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)
          ],
          "{forgotten:?}"
      );
      // The unlisted day's rollout and the locked day's.
      assert_eq!(count_left(&forgotten, ForgetKind::Transcript), 2, "{forgotten:?}");
      assert_eq!(removed(&forgotten, ForgetKind::Transcript), 1, "only the archived one");
      assert!(root.exists(OWN[0]) && root.exists(OWN[1]));
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test forget_codex`
Expected: FAIL. Codex's forget still answers `unsupported_agent`.

- [ ] **Step 3: `thread/delete`, the fallback walk, and the manifest pin**

  In `adapters/manifest.json`, replace:

  ```json
      }
    }
  ```

  with:

  ```json
      }
    },
    "codex_app_server": {
      "codex_version": "0.155.1",
      "bin": "node_modules/@openai/codex/bin/codex.js",
      "initialize": {
        "clientInfo": {
          "name": "hennery",
          "title": "hennery",
          "version": "1"
        }
      },
      "delete_method": "thread/delete",
      "params_shape": {
        "threadId": "{thread_id}"
      }
    }
  ```

  In `adapters/pins.toml`, replace:

  ```toml
  # e2e gate (umbrella §14) is the operator's to run, with logged-in agents.
  ```

  with:

  ```toml
  # e2e gate (umbrella §14) is the operator's to run, with logged-in agents,
  # and so is the checklist in packaging/README.md ("Pin bumps").
  ```

  In `adapters/pins.toml`, replace:

  ```toml
  cli = ["node_modules/@openai/codex-"]
  ```

  with:

  ```toml
  cli = ["node_modules/@openai/codex-"]

  # How a forget deletes a Codex session's thread (plan 9d decision 9):
  # `<node> <bin> app-server`, `initialize`, then `thread/delete`, with the
  # call shape read from this Codex version's source (`rust-v0.155.1`,
  # `app-server-protocol`). The generator refuses it unless the codex
  # lockfile bundles exactly `codex_version`; a bump re-reads the shape, and
  # the live check in packaging/README.md ("Pin bumps") is the operator's.
  [codex_app_server]
  codex_version = "0.155.1"
  bin = "node_modules/@openai/codex/bin/codex.js"
  delete_method = "thread/delete"
  params_shape = { threadId = "{thread_id}" }

  [codex_app_server.initialize.clientInfo]
  name = "hennery"
  title = "hennery"
  version = "1"
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
      pub profiles: HashMap<String, Profile>,
      pub reconnect_min: Duration,
  ```

  with:

  ```rust
      pub profiles: HashMap<String, Profile>,
      /// The Codex CLI a forget runs `app-server` from (plan 9d decision 9):
      /// the installed set's bundled one, or `--use-cli`'s. `None` (no set,
      /// or `--agent`): a Codex forget takes the fallback.
      pub codex_app_server: Option<AgentCommand>,
      pub reconnect_min: Duration,
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              profiles: HashMap::new(),
              reconnect_min: Duration::from_millis(500),
  ```

  with:

  ```rust
              profiles: HashMap::new(),
              codex_app_server: None,
              reconnect_min: Duration::from_millis(500),
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
          account: None,
      };
  ```

  with:

  ```rust
          account: None,
          codex_app_server: cfg.codex_app_server.clone(),
          codex_pin: crate::runtime::manifest::Manifest::embedded().codex_app_server,
          deadline: crate::forget::FORGET_DEADLINE,
      };
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      pub account: Option<Account>,
  }
  ```

  with:

  ```rust
      pub account: Option<Account>,
      /// The Codex CLI a forget runs `app-server` from (plan 9d decision 9):
      /// the set's bundled one, or `--use-cli`'s. `None`: the fallback only.
      pub codex_app_server: Option<AgentCommand>,
      /// `thread/delete`'s call shape, pinned for one Codex version (the
      /// embedded manifest's `codex_app_server`). `None`: the fallback only.
      pub codex_pin: Option<crate::runtime::manifest::CodexAppServer>,
      /// The whole forget's deadline (B6): `FORGET_DEADLINE` on a real host.
      pub deadline: Duration,
  }
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  /// Run one forget (plan 9d decision 8) within `FORGET_DEADLINE`. Only
  /// Claude's data is removed so far; any other agent is answered
  /// `unsupported_agent`, retryable, for plan 9d-ii to take up.
  pub async fn forget(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      if forget.agent != crate::agent_home::CLAUDE {
  ```

  with:

  ```rust
  /// Run one forget within the context's deadline: Claude's data (plan 9d
  /// decision 8) or Codex's (plan 9d-ii, decisions 9 and 10). Any other agent
  /// is answered `unsupported_agent`, retryable.
  pub async fn forget(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      let agent = forget.agent.as_str();
      if agent != crate::agent_home::CLAUDE && agent != crate::agent_home::CODEX {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      }
      forget_claude(ctx, forget).await
  ```

  with:

  ```rust
      }
      if agent == crate::agent_home::CODEX {
          return crate::forget_codex::forget_codex(ctx, forget).await;
      }
      forget_claude(ctx, forget).await
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      let until = Instant::now() + FORGET_DEADLINE;
      let root = PathBuf::from(&forget.agent_home.root);
      let checked = {
          let (ctx, root) = (ctx.clone(), root.clone());
          tokio::task::spawn_blocking(move || check(&ctx, &root)).await
  ```

  with:

  ```rust
      let until = Instant::now() + ctx.deadline;
      let root = PathBuf::from(&forget.agent_home.root);
      let checked = {
          let (ctx, root) = (ctx.clone(), root.clone());
          tokio::task::spawn_blocking(move || check(&ctx, &root, &CLAUDE_KINDS)).await
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
          run_adapter(ctx, forget, &root, until).await;
  ```

  with:

  ```rust
          let env = vec![("CLAUDE_CONFIG_DIR".to_string(), root.to_string_lossy().into_owned())];
          run_adapter(ctx, forget, &root, env, FORGET_STRIPPED_VARS, until).await;
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
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
  ```

  with:

  ```rust
  pub(crate) const ADAPTER_SHARE: Duration = Duration::from_secs(12);

  /// The grace a forget's adapter gets between SIGTERM and SIGKILL.
  pub(crate) const ADAPTER_GRACE: Duration = Duration::from_secs(2);

  /// The agent's own delete (decision 8; for Codex, decision 10's archive):
  /// its adapter, through `Adapter::spawn`'s hygiene (B6), with `env` set
  /// (the root, as the agent's own variable) and `strip` removed, the root
  /// as its cwd, then `initialize`, then `session/delete` if it advertises
  /// it. Whatever it answers (not found included) counts for nothing: the
  /// check afterwards decides (B4). Outside the no-follow guarantee: the
  /// adapter resolves its own paths. Its group is killed after, within the
  /// deadline.
  pub(crate) async fn run_adapter(
      ctx: &ForgetContext,
      forget: &Forget,
      root: &Path,
      env: Vec<(String, String)>,
      strip: &[&str],
      until: Instant,
  ) {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      command
          .env
          .push(("CLAUDE_CONFIG_DIR".into(), root.to_string_lossy().into_owned()));
      let (mut adapter, io) = match Adapter::spawn_stripped(&command, root, FORGET_STRIPPED_VARS) {
  ```

  with:

  ```rust
      command.env.extend(env);
      let (mut adapter, io) = match Adapter::spawn_stripped(&command, root, strip) {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  /// still there, or a removal that failed midway (B3). A symlink, a mount
  /// point, an unsafe directory stay as they are.
  fn retryable(reason: ForgetReason) -> bool {
  ```

  with:

  ```rust
  /// still there, or a removal that failed midway (B3), a deadline, another
  /// forget or a live worker. A symlink, a mount point, an unsafe directory,
  /// Codex's refusals for forked or unpersisted history, another home and
  /// what only the fallback could reach stay as they are.
  pub(crate) fn retryable(reason: ForgetReason) -> bool {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  struct Tally {
      removed: BTreeMap<ForgetKind, u32>,
  ```

  with:

  ```rust
  pub(crate) struct Tally {
      pub(crate) removed: BTreeMap<ForgetKind, u32>,
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      fn left(&mut self, kind: ForgetKind, reason: ForgetReason) {
          *self.left.entry((kind, reason)).or_default() += 1;
      }

      fn into_forgotten(self) -> Forgotten {
  ```

  with:

  ```rust
      pub(crate) fn left(&mut self, kind: ForgetKind, reason: ForgetReason) {
          *self.left.entry((kind, reason)).or_default() += 1;
      }

      /// `kind` left for `reason` with no count of its own (a whole forget,
      /// or a database): counted once, as 0.
      pub(crate) fn left_whole(&mut self, kind: ForgetKind, reason: ForgetReason) {
          self.left.entry((kind, reason)).or_default();
      }

      pub(crate) fn into_forgotten(self) -> Forgotten {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  fn stop_reason(stop: walk::Stop) -> ForgetReason {
  ```

  with:

  ```rust
  pub(crate) fn stop_reason(stop: walk::Stop) -> ForgetReason {
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
  type KindDir = (ForgetKind, &'static str, Result<Option<OwnedFd>, ForgetReason>);

  /// The checked root's kind directories, as opened before the adapter ran.
  struct Kinds {
      root: OwnedFd,
      dev: libc::dev_t,
      dirs: Vec<KindDir>,
  }

  /// The root and its kind directories, checked (B3). `Err` for the root.
  /// Runs on a blocking thread: the account lookup with it.
  fn check(ctx: &ForgetContext, root: &Path) -> Result<Kinds, ForgetReason> {
      let me = ctx.account.clone().unwrap_or_else(account);
      let (root_fd, dev) = open_checked_root(ctx, root, &me)?;
      let dirs = CLAUDE_KINDS
  ```

  with:

  ```rust
  pub(crate) type KindDir = (ForgetKind, &'static str, Result<Option<OwnedFd>, ForgetReason>);

  /// The checked root's kind directories, as opened before the adapter ran.
  pub(crate) struct Kinds {
      pub(crate) root: OwnedFd,
      pub(crate) dev: libc::dev_t,
      pub(crate) dirs: Vec<KindDir>,
  }

  /// The root and the agent's kind directories, checked (B3). `Err` for the
  /// root. Runs on a blocking thread: the account lookup with it.
  pub(crate) fn check(
      ctx: &ForgetContext,
      root: &Path,
      kinds: &[(ForgetKind, &'static str)],
  ) -> Result<Kinds, ForgetReason> {
      let me = ctx.account.clone().unwrap_or_else(account);
      let (root_fd, dev) = open_checked_root(ctx, root, &me)?;
      let dirs = kinds
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
              account: None,
          };
  ```

  with:

  ```rust
              account: None,
              codex_app_server: None,
              codex_pin: None,
              deadline: FORGET_DEADLINE,
          };
  ```

  Create `crates/hennery-host/src/forget_codex.rs`:

  ```rust
  //! `forget_session` for Codex on the host (plan 9d decisions 9, 10, 12;
  //! B3–B6): the session's thread deleted by Codex's own `thread/delete`,
  //! through the app-server under the session's recorded `CODEX_HOME` and
  //! never another, and, only when that path is unavailable, the fallback:
  //! codex-acp's archive, then the rollout files through the descriptor walk.
  //!
  //! The order:
  //! 1. **Check** the root and its `sessions/` and `archived_sessions/` (B3).
  //!    One that fails its check (a symlink: a composed home's linked
  //!    `sessions/`, decision 12) is reported, and nothing is spawned:
  //!    `thread/delete` and the archive would both follow it.
  //! 2. **The app-server** (decision 9): the binary's `--version` must be the
  //!    one the manifest pins a call shape for; then `app-server`, through
  //!    `Adapter::spawn`'s hygiene (B6) with `CODEX_HOME` the root and
  //!    `CODEX_SQLITE_HOME` the recorded one or removed, the root its cwd;
  //!    then the pinned `initialize`, whose `codexHome` must be the root; then
  //!    `thread/delete {threadId}`. Its group is killed after, and the whole
  //!    call has one deadline.
  //! 3. **The outcome** is decided by checking afterwards that no rollout of
  //!    the session is left in `sessions/` or `archived_sessions/` (B4). The
  //!    SQLite copies, `thread_history` and `session_index.jsonl` are
  //!    `thread/delete`'s contract: hennery does not verify them.
  //! 4. **The fallback** (decision 10, B5) runs when the app-server path is
  //!    unavailable: no binary or no pin, a spawn that fails, a version with
  //!    no pinned shape, an `initialize` that fails, or a `thread/delete`
  //!    this Codex does not know. Never after `thread/delete` itself
  //!    answered: a refusal, a failure, or no answer in time. Nor when a step
  //!    only ran out of time (a first exec macOS scans, say): that is
  //!    `timed_out`, retried, since the fallback's result is final.

  use crate::adapter::{Adapter, AdapterIo, AgentCommand};
  use crate::forget::{
      ADAPTER_GRACE, Forget, ForgetContext, Forgotten, KindDir, Kinds, Tally, check, left, retryable, valid_id,
  };
  use crate::runtime::manifest::CodexAppServer;
  use crate::walk;
  use hennery_proto::frames::{ForgetKind, ForgetReason};
  use std::collections::BTreeMap;
  use std::ffi::CString;
  use std::os::fd::{AsRawFd, RawFd};
  use std::os::unix::ffi::OsStrExt;
  use std::path::{Path, PathBuf};
  use std::sync::Arc;
  use std::time::Duration;
  use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
  use tokio::time::Instant;

  /// The kind directories under a Codex root (Codex's `SESSIONS_SUBDIR` and
  /// `ARCHIVED_SESSIONS_SUBDIR`), both holding rollouts: the transcript.
  const CODEX_KINDS: [(ForgetKind, &str); 2] = [
      (ForgetKind::Transcript, "sessions"),
      (ForgetKind::Transcript, "archived_sessions"),
  ];

  /// How deep the walk goes under `sessions/` (Codex's `YYYY/MM/DD`), and
  /// under `archived_sessions/` (its top only: Codex archives flat).
  const SESSIONS_DEPTH: usize = 3;
  const ARCHIVED_DEPTH: usize = 0;

  /// The app-server's share of a forget's deadline (B6): its version, its
  /// `initialize` and `thread/delete` all within it. The rest is the
  /// fallback's and the walk's, so a hung app-server leaves the fallback its
  /// time.
  fn app_server_share(deadline: Duration) -> Duration {
      deadline * 2 / 5
  }

  /// The longest line the host reads from the app-server: an `initialize` or
  /// `thread/delete` answer is a few hundred bytes.
  const MAX_LINE: u64 = 1 << 20;

  /// The most a `--version` prints that is read.
  const MAX_VERSION: u64 = 4096;

  /// Variables a forget's Codex processes never inherit unless recorded
  /// (B6): another project directory name is Claude's, and another SQLite
  /// home would aim Codex at other database copies.
  const CLAUDE_PROJECT_VAR: &str = "CLAUDE_CODE_PROJECT_DIR_NAME";
  const SQLITE_VAR: &str = "CODEX_SQLITE_HOME";

  /// What the app-server path came to.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  enum Verdict {
      /// `thread/delete` answered success, or that there is no such thread:
      /// the check afterwards decides.
      Deleted,
      /// `thread/delete` refused, for this reason: nothing of the thread was
      /// deleted, and the fallback never runs after it (B5).
      Refused(ForgetReason),
      /// `thread/delete` was sent and did not finish: it failed midway, or
      /// gave no answer in time. Retried; no fallback (it may have run).
      Failed(ForgetReason),
      /// The app-server resolved another `CODEX_HOME` than the recorded one:
      /// nothing more was asked of it, and no fallback runs, since codex-acp
      /// would resolve the same home (the parent's rule).
      HomeMismatch,
      /// The app-server path is unavailable, for this reason: the fallback
      /// runs (B5).
      Unavailable(Unavailable),
  }

  /// Why the app-server path is unavailable: each a fallback trigger (B5).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  enum Unavailable {
      /// No Codex binary to run (no set, no bundled CLI, no `--use-cli`).
      NoBinary,
      /// The manifest pins no call shape.
      NoPin,
      /// The binary did not spawn, or its `--version` failed.
      Spawn,
      /// Its `--version` is not the one the manifest pins a shape for.
      Unpinned,
      /// `initialize` failed: an error, or the app-server's end.
      Initialize,
      /// This Codex does not know `thread/delete` (`-32601`, or `-32600
      /// Invalid request`: how 0.155.1 answers a method or params it cannot
      /// read, before any handler runs).
      MethodNotFound,
  }

  /// An answer to one request, as the app-server sent it.
  #[derive(Debug, Clone, PartialEq)]
  enum Answer {
      Result(serde_json::Value),
      Error { code: i64, message: String },
  }

  /// `thread/delete`'s answer, as Codex 0.155.1 gives it (its
  /// `thread_delete.rs`, `delete_thread.rs`, `thread_manager.rs` and
  /// `message_processor.rs` at `rust-v0.155.1`). The text only picks the
  /// reason: what is left is decided by the check afterwards (B4).
  fn classify_delete(answer: &Answer) -> Verdict {
      let (code, message) = match answer {
          Answer::Result(_) => return Verdict::Deleted,
          Answer::Error { code, message } => (*code, message.as_str()),
      };
      match code {
          -32601 => Verdict::Unavailable(Unavailable::MethodNotFound),
          // The request did not deserialize: an unknown method, or params of
          // another shape. No handler ran.
          -32600 if message.starts_with("Invalid request: ") => Verdict::Unavailable(Unavailable::MethodNotFound),
          // Nothing of the thread is there (0.155.1 says either).
          -32600 if message.starts_with("no rollout found for thread id") || message.starts_with("thread not found:") => {
              Verdict::Deleted
          }
          -32600 if message.contains("forked history still references it") => {
              Verdict::Refused(ForgetReason::ForkedHistory)
          }
          -32600 if message.starts_with("thread is not persisted") => Verdict::Refused(ForgetReason::Ephemeral),
          // A live internal worker: its owner releases it, so a retry may
          // find it gone.
          -32600 if message.starts_with("live internal threads") => Verdict::Refused(ForgetReason::InProgress),
          _ => Verdict::Failed(ForgetReason::IoError),
      }
  }

  /// The version a Codex CLI's `--version` prints (`codex-cli 0.155.1`).
  fn parse_version(out: &str) -> Option<&str> {
      let version = out.lines().next()?.strip_prefix("codex-cli ")?.trim();
      let plain = version.split('.').count() == 3
          && version
              .split('.')
              .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
      plain.then_some(version)
  }

  /// Whether `name` is one of thread `id`'s rollout files, as Codex's
  /// `recorder.rs` and `rollout_file_name.rs` name them, anchored at both
  /// ends (the id checked by `valid_id`):
  ///
  /// `^rollout-[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}-[0-9]{2}-[0-9]{2}-<id>(_<uuid>)?\.jsonl(\.zst)?$`
  ///
  /// The `_<uuid>` is a reverted thread's rollout id; the thread id is
  /// always the first, so another thread's rollout whose rollout id is `id`
  /// does not match.
  pub fn is_rollout_of(name: &[u8], id: &str) -> bool {
      let Some(rest) = name.strip_prefix(b"rollout-") else {
          return false;
      };
      let Some((stamp, rest)) = rest.split_at_checked(19) else {
          return false;
      };
      let stamp_ok = stamp.iter().enumerate().all(|(i, b)| match i {
          4 | 7 | 13 | 16 => *b == b'-',
          10 => *b == b'T',
          _ => b.is_ascii_digit(),
      });
      let Some(rest) = rest.strip_prefix(b"-").and_then(|r| r.strip_prefix(id.as_bytes())) else {
          return false;
      };
      let rest = rest.strip_suffix(b".zst").unwrap_or(rest);
      let Some(rest) = rest.strip_suffix(b".jsonl") else {
          return false;
      };
      let rollout_ok = match rest.strip_prefix(b"_") {
          None => rest.is_empty(),
          Some(rollout) => std::str::from_utf8(rollout).is_ok_and(valid_id),
      };
      stamp_ok && valid_id(id) && rollout_ok
  }

  /// Whether `name` is a date directory at `depth` under `sessions/`: `YYYY`
  /// at 1, then `MM` and `DD`. Nothing else there is Codex's.
  fn is_date_dir(name: &[u8], depth: usize) -> bool {
      let len = if depth == 1 { 4 } else { 2 };
      name.len() == len && name.iter().all(u8::is_ascii_digit)
  }

  /// One forget of a Codex session (decisions 9 and 10).
  pub async fn forget_codex(ctx: &ForgetContext, forget: &Forget) -> Forgotten {
      let start = Instant::now();
      let until = start + ctx.deadline;
      let root = PathBuf::from(&forget.agent_home.root);
      let checked = {
          let (ctx, root) = (ctx.clone(), root.clone());
          tokio::task::spawn_blocking(move || check(&ctx, &root, &CODEX_KINDS)).await
      };
      let kinds = match checked {
          Ok(Ok(kinds)) => Arc::new(kinds),
          Ok(Err(reason)) => return whole(reason),
          Err(_) => return whole(ForgetReason::IoError),
      };
      // B3: a kind directory that failed its check stops everything here.
      let refused: Vec<(ForgetKind, ForgetReason)> = kinds
          .dirs
          .iter()
          .filter_map(|(kind, _, opened)| opened.as_ref().err().map(|reason| (*kind, *reason)))
          .collect();
      if !refused.is_empty() {
          let mut tally = Tally::default();
          for (kind, reason) in refused {
              tally.left(kind, reason);
          }
          return tally.into_forgotten();
      }
      let (env, strip) = environment(forget, &root);
      // What is there before, so what `thread/delete` removes can be
      // counted: only if the app-server may run.
      let before = if ctx.codex_pin.is_some() && ctx.codex_app_server.is_some() {
          let (kinds, id, hooks) = (kinds.clone(), forget.agent_session_id.clone(), ctx.hooks.clone());
          tokio::task::spawn_blocking(move || present(&kinds, &id, &hooks))
              .await
              .unwrap_or(0)
      } else {
          0
      };
      let share = until.min(start + app_server_share(ctx.deadline));
      let verdict = app_server(ctx, forget, &root, &env, &strip, share).await;
      let id = forget.agent_session_id.clone();
      match verdict {
          Verdict::Unavailable(why) => {
              tracing::info!(
                  ?why,
                  "Codex's app-server is unavailable for a forget; the fallback runs"
              );
              crate::forget::run_adapter(ctx, forget, &root, env, &strip, until).await;
              let hooks = ctx.hooks.clone();
              let walked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent);
                  // Decision 10: what only `thread/delete` reaches.
                  tally.left_whole(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly);
                  tally
              })
              .await;
              match walked {
                  Ok(tally) => tally.into_forgotten(),
                  Err(_) => whole(ForgetReason::IoError),
              }
          }
          Verdict::HomeMismatch => whole(ForgetReason::HomeMismatch),
          Verdict::Deleted | Verdict::Refused(_) | Verdict::Failed(_) => {
              let (left_as, the_delete) = match verdict {
                  Verdict::Refused(reason) | Verdict::Failed(reason) => (reason, Some(reason)),
                  _ => (ForgetReason::StillPresent, None),
              };
              let hooks = ctx.hooks.clone();
              let checked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, false, left_as);
                  let after = present(&kinds, &id, &hooks);
                  if before > after {
                      tally.removed.insert(ForgetKind::Transcript, before - after);
                  }
                  if let Some(reason) = the_delete {
                      tally.left_whole(ForgetKind::Session, reason);
                  }
                  tally
              })
              .await;
              match checked {
                  Ok(tally) => tally.into_forgotten(),
                  Err(_) => whole(ForgetReason::IoError),
              }
          }
      }
  }

  /// The whole forget left, for `reason`.
  fn whole(reason: ForgetReason) -> Forgotten {
      Forgotten {
          removed: Vec::new(),
          remaining: vec![left(ForgetKind::Session, 0, reason, retryable(reason))],
      }
  }

  /// What a forget's Codex processes run with (B6, the parent's rule):
  /// `CODEX_HOME` the recorded root, `CODEX_SQLITE_HOME` the recorded one;
  /// and what is removed: that one when none is recorded, and Claude's
  /// project-name override always.
  fn environment(forget: &Forget, root: &Path) -> (Vec<(String, String)>, Vec<&'static str>) {
      let mut env = vec![("CODEX_HOME".to_string(), root.to_string_lossy().into_owned())];
      let mut strip = vec![CLAUDE_PROJECT_VAR];
      match &forget.agent_home.sqlite_root {
          Some(sqlite) => env.push((SQLITE_VAR.to_string(), sqlite.clone())),
          None => strip.push(SQLITE_VAR),
      }
      (env, strip)
  }

  /// The app-server path (decision 9), within `until`.
  async fn app_server(
      ctx: &ForgetContext,
      forget: &Forget,
      root: &Path,
      env: &[(String, String)],
      strip: &[&str],
      until: Instant,
  ) -> Verdict {
      let Some(pin) = &ctx.codex_pin else {
          return Verdict::Unavailable(Unavailable::NoPin);
      };
      let Some(binary) = &ctx.codex_app_server else {
          return Verdict::Unavailable(Unavailable::NoBinary);
      };
      let command = |arg: &str| {
          let mut command = binary.clone();
          command.args.push(arg.to_string());
          command.env.extend(env.iter().cloned());
          command
      };
      match version(&command("--version"), root, strip, until).await {
          Ok(Some(version)) if version == pin.codex_version => {}
          Ok(version) => {
              tracing::info!(?version, pinned = %pin.codex_version, "Codex's version has no pinned delete");
              return Verdict::Unavailable(Unavailable::Unpinned);
          }
          Err(verdict) => return verdict,
      }
      let (mut process, io) = match Adapter::spawn_stripped(&command("app-server"), root, strip) {
          Ok(spawned) => spawned,
          Err(err) => {
              tracing::warn!(error = %err, "Codex's app-server did not spawn");
              return Verdict::Unavailable(Unavailable::Spawn);
          }
      };
      let verdict = talk(io, pin, root, &forget.agent_session_id, until).await;
      process.terminate(ADAPTER_GRACE).await;
      verdict
  }

  /// A Codex CLI's `--version`, through the same hygiene, within `until`:
  /// the version it printed, if any; the verdict if it did not run, or ran
  /// out of time.
  async fn version(
      command: &AgentCommand,
      root: &Path,
      strip: &[&str],
      until: Instant,
  ) -> Result<Option<String>, Verdict> {
      let (mut process, io) = match Adapter::spawn_stripped(command, root, strip) {
          Ok(spawned) => spawned,
          Err(err) => {
              tracing::warn!(error = %err, "Codex's CLI did not spawn");
              return Err(Verdict::Unavailable(Unavailable::Spawn));
          }
      };
      let AdapterIo { stdin, stdout } = io;
      drop(stdin);
      let mut out = Vec::new();
      let read = tokio::time::timeout_at(until, stdout.take(MAX_VERSION).read_to_end(&mut out)).await;
      process.terminate(ADAPTER_GRACE).await;
      match read {
          Err(_) => Err(Verdict::Failed(ForgetReason::TimedOut)),
          Ok(Err(_)) => Err(Verdict::Unavailable(Unavailable::Spawn)),
          Ok(Ok(_)) => Ok(parse_version(&String::from_utf8_lossy(&out)).map(str::to_string)),
      }
  }

  /// `initialize`, its `codexHome` checked, then `thread/delete` (decision
  /// 9): one JSON object per line, no `jsonrpc` field, as codex-acp speaks
  /// to it. Notifications and requests from the app-server are read past.
  async fn talk(io: AdapterIo, pin: &CodexAppServer, root: &Path, id: &str, until: Instant) -> Verdict {
      let AdapterIo { mut stdin, stdout } = io;
      let mut reader = BufReader::new(stdout);
      let initialize = serde_json::json!({ "id": 1, "method": "initialize", "params": pin.initialize });
      if send(&mut stdin, &initialize).await.is_err() {
          return Verdict::Unavailable(Unavailable::Initialize);
      }
      match tokio::time::timeout_at(until, answer(&mut reader, 1)).await {
          Ok(Some(Answer::Result(result))) => {
              // The home it resolved is the recorded one, or nothing more is
              // asked of it.
              if result.get("codexHome").and_then(|h| h.as_str()) != Some(root.to_string_lossy().as_ref()) {
                  tracing::warn!("Codex's app-server resolved another CODEX_HOME than the recorded one");
                  return Verdict::HomeMismatch;
              }
          }
          Ok(Some(Answer::Error { code, message })) => {
              tracing::info!(code, %message, "Codex's app-server refused initialize");
              return Verdict::Unavailable(Unavailable::Initialize);
          }
          Ok(None) => return Verdict::Unavailable(Unavailable::Initialize),
          Err(_) => return Verdict::Failed(ForgetReason::TimedOut),
      }
      let delete = serde_json::json!({ "id": 2, "method": pin.delete_method, "params": pin.params(id) });
      if send(&mut stdin, &delete).await.is_err() {
          return Verdict::Failed(ForgetReason::IoError);
      }
      match tokio::time::timeout_at(until, answer(&mut reader, 2)).await {
          Ok(Some(answer)) => {
              let verdict = classify_delete(&answer);
              if let Answer::Error { code, message } = &answer {
                  tracing::info!(code, %message, ?verdict, "Codex's thread/delete answered an error");
              }
              verdict
          }
          Ok(None) => Verdict::Failed(ForgetReason::IoError),
          Err(_) => Verdict::Failed(ForgetReason::TimedOut),
      }
  }

  async fn send(stdin: &mut tokio::process::ChildStdin, frame: &serde_json::Value) -> std::io::Result<()> {
      let mut line = serde_json::to_vec(frame).expect("a frame serialises");
      line.push(b'\n');
      stdin.write_all(&line).await?;
      stdin.flush().await
  }

  /// The answer to request `want`: lines read until one carries that id and
  /// a result or an error. `None` at the end of the stream, on a read error
  /// or past `MAX_LINE`.
  async fn answer<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R, want: u64) -> Option<Answer> {
      loop {
          let mut line = Vec::new();
          let n = (&mut *reader).take(MAX_LINE).read_until(b'\n', &mut line).await.ok()?;
          if n == 0 || (line.last() != Some(&b'\n') && n as u64 >= MAX_LINE) {
              return None;
          }
          let Ok(message) = serde_json::from_slice::<serde_json::Value>(&line) else {
              continue;
          };
          if message.get("method").is_some() || message.get("id").and_then(|i| i.as_u64()) != Some(want) {
              continue;
          }
          if let Some(error) = message.get("error") {
              return Some(Answer::Error {
                  code: error.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                  message: error
                      .get("message")
                      .and_then(|m| m.as_str())
                      .unwrap_or_default()
                      .to_string(),
              });
          }
          if let Some(result) = message.get("result") {
              return Some(Answer::Result(result.clone()));
          }
      }
  }

  /// The session's rollouts under the checked kind directories (decision 10,
  /// B5): with `remove`, each regular file is unlinked first (never a
  /// symlink, never through one); then they are counted again, what is still
  /// there left for `left_as` (or why its removal failed), a symlink for
  /// `symlink`. The check afterwards decides (B4).
  fn rollouts(kinds: &Kinds, id: &str, hooks: &walk::Hooks, remove: bool, left_as: ForgetReason) -> Tally {
      let mut tally = Tally::default();
      // Why a removal failed, by the directory's device and inode and name.
      let mut failed: BTreeMap<(u64, u64, Vec<u8>), ForgetReason> = BTreeMap::new();
      if remove {
          let mut scratch = Tally::default();
          for (dir, path, depth) in kind_dirs(kinds) {
              let walker = Walk {
                  id,
                  dev: kinds.dev,
                  hooks,
                  max: depth,
              };
              walker.dir(
                  dir,
                  &path,
                  0,
                  &mut |dir, name, st, tally| {
                      if walk::is_file(st) {
                          match walk::unlink_file_at(dir, name) {
                              Ok(()) => *tally.removed.entry(ForgetKind::Transcript).or_default() += 1,
                              Err(_) => {
                                  failed.insert(key(dir, name), ForgetReason::IoError);
                              }
                          }
                      }
                  },
                  &mut scratch,
              );
          }
          tally.removed = scratch.removed;
      }
      for (dir, path, depth) in kind_dirs(kinds) {
          let walker = Walk {
              id,
              dev: kinds.dev,
              hooks,
              max: depth,
          };
          walker.dir(
              dir,
              &path,
              0,
              &mut |dir, name, st, tally| {
                  let reason = if walk::is_link(st) {
                      ForgetReason::Symlink
                  } else if walk::is_file(st) {
                      failed.get(&key(dir, name)).copied().unwrap_or(left_as)
                  } else {
                      ForgetReason::StillPresent
                  };
                  tally.left(ForgetKind::Transcript, reason);
              },
              &mut tally,
          );
      }
      tally
  }

  /// How many of the session's rollout files (regular files) are there.
  fn present(kinds: &Kinds, id: &str, hooks: &walk::Hooks) -> u32 {
      let mut count = 0;
      let mut scratch = Tally::default();
      for (dir, path, depth) in kind_dirs(kinds) {
          let walker = Walk {
              id,
              dev: kinds.dev,
              hooks,
              max: depth,
          };
          walker.dir(
              dir,
              &path,
              0,
              &mut |_, _, st, _| {
                  if walk::is_file(st) {
                      count += 1;
                  }
              },
              &mut scratch,
          );
      }
      count
  }

  /// The open kind directories, each with its path (for the hooks only) and
  /// how deep the walk goes in it.
  fn kind_dirs(kinds: &Kinds) -> Vec<(RawFd, PathBuf, usize)> {
      kinds
          .dirs
          .iter()
          .filter_map(|(_, name, opened): &KindDir| {
              let Ok(Some(fd)) = opened else {
                  return None;
              };
              let depth = if *name == "sessions" {
                  SESSIONS_DEPTH
              } else {
                  ARCHIVED_DEPTH
              };
              Some((fd.as_raw_fd(), PathBuf::from(name), depth))
          })
          .collect()
  }

  /// A rollout's identity for the second pass: its directory's device and
  /// inode, and its name.
  fn key(dir: RawFd, name: &CString) -> (u64, u64, Vec<u8>) {
      // The field types differ by platform (`st_dev` is `i32` on macOS).
      #[allow(clippy::unnecessary_cast)]
      let (dev, ino) = walk::stat_fd(dir).map_or((0, 0), |st| (st.st_dev as u64, st.st_ino as u64));
      (dev, ino, name.as_bytes().to_vec())
  }

  /// One walk of a kind directory, through descriptors only (B3, R1, R2):
  /// each date directory opened with `O_NOFOLLOW` relative to its parent,
  /// on the root's file system, never past `max` levels.
  struct Walk<'a> {
      id: &'a str,
      dev: libc::dev_t,
      hooks: &'a walk::Hooks,
      max: usize,
  }

  /// Called for each entry named as one of the session's rollouts, with its
  /// directory, its name and what `fstatat(AT_SYMLINK_NOFOLLOW)` found.
  type OnRollout<'f> = dyn FnMut(RawFd, &CString, &libc::stat, &mut Tally) + 'f;

  impl Walk<'_> {
      fn dir(&self, dir: RawFd, path: &Path, depth: usize, on: &mut OnRollout<'_>, tally: &mut Tally) {
          let names = match self.hooks.list(dir, path) {
              Ok(names) => names,
              Err(_) => return tally.left(ForgetKind::Transcript, ForgetReason::IoError),
          };
          self.hooks.listed(path, &names);
          for name in names {
              let st = match walk::stat_at(dir, &name) {
                  Ok(Some(st)) => st,
                  Ok(None) => continue,
                  Err(_) => {
                      tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                      continue;
                  }
              };
              if is_rollout_of(name.as_bytes(), self.id) {
                  on(dir, &name, &st, tally);
                  continue;
              }
              if depth >= self.max || !is_date_dir(name.as_bytes(), depth + 1) {
                  continue;
              }
              // A date directory that is a symlink is reported, never entered.
              if walk::is_link(&st) {
                  tally.left(ForgetKind::Transcript, ForgetReason::Symlink);
                  continue;
              }
              if !walk::is_dir(&st) {
                  continue;
              }
              let child_path = path.join(std::ffi::OsStr::from_bytes(name.as_bytes()));
              self.hooks.stated(&child_path);
              let child = match walk::open_dir_at(dir, &name) {
                  Ok(fd) => fd,
                  // Swapped for a link since it was looked at (R3).
                  Err(libc::ELOOP | libc::ENOTDIR) => {
                      if matches!(walk::stat_at(dir, &name), Ok(Some(st)) if walk::is_link(&st)) {
                          tally.left(ForgetKind::Transcript, ForgetReason::Symlink);
                      }
                      continue;
                  }
                  Err(libc::ENOENT) => continue,
                  Err(_) => {
                      tally.left(ForgetKind::Transcript, ForgetReason::IoError);
                      continue;
                  }
              };
              match walk::stat_fd(child.as_raw_fd()) {
                  Ok(st) if st.st_dev == self.dev => self.dir(child.as_raw_fd(), &child_path, depth + 1, on, tally),
                  Ok(_) => tally.left(ForgetKind::Transcript, ForgetReason::MountPoint),
                  Err(_) => tally.left(ForgetKind::Transcript, ForgetReason::IoError),
              }
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      const ID: &str = "019a0b1c-2d3e-7f40-8a5b-6c7d8e9f0a1b";

      fn error(code: i64, message: &str) -> Answer {
          Answer::Error {
              code,
              message: message.into(),
          }
      }

      /// Decision 9, B5, the fleet's per-outcome rule: each answer
      /// `thread/delete` can give maps to its verdict.
      #[test]
      fn each_answer_of_thread_delete_has_its_verdict() {
          assert_eq!(
              classify_delete(&Answer::Result(serde_json::json!({}))),
              Verdict::Deleted
          );
          assert_eq!(
              classify_delete(&error(-32600, &format!("no rollout found for thread id {ID}"))),
              Verdict::Deleted
          );
          assert_eq!(
              classify_delete(&error(-32600, &format!("thread not found: {ID}"))),
              Verdict::Deleted
          );
          assert_eq!(
              classify_delete(&error(
                  -32600,
                  &format!("cannot delete thread {ID}: forked history still references it")
              )),
              Verdict::Refused(ForgetReason::ForkedHistory)
          );
          assert_eq!(
              classify_delete(&error(
                  -32600,
                  &format!("thread is not persisted and cannot be deleted: {ID}")
              )),
              Verdict::Refused(ForgetReason::Ephemeral)
          );
          assert_eq!(
              classify_delete(&error(
                  -32600,
                  "live internal threads can only be removed by their owner"
              )),
              Verdict::Refused(ForgetReason::InProgress)
          );
          assert_eq!(
              classify_delete(&error(-32601, "thread/delete is not supported yet")),
              Verdict::Unavailable(Unavailable::MethodNotFound)
          );
          assert_eq!(
              classify_delete(&error(
                  -32600,
                  "Invalid request: unknown variant `thread/delete`, expected one of `initialize`"
              )),
              Verdict::Unavailable(Unavailable::MethodNotFound)
          );
          assert_eq!(
              classify_delete(&error(-32600, "Invalid request: missing field `threadId`")),
              Verdict::Unavailable(Unavailable::MethodNotFound)
          );
          for other in [
              error(-32603, "failed to delete thread: database is locked"),
              error(-32600, "failed to locate thread id x: permission denied"),
              error(-32001, "Server overloaded; retry later."),
          ] {
              assert_eq!(
                  classify_delete(&other),
                  Verdict::Failed(ForgetReason::IoError),
                  "{other:?}"
              );
          }
      }

      /// Every reason the Codex path gives is retried only when a retry can
      /// change it (decision 4).
      #[test]
      fn only_a_live_worker_a_failure_or_a_deadline_is_retried() {
          for (reason, retried) in [
              (ForgetReason::ForkedHistory, false),
              (ForgetReason::Ephemeral, false),
              (ForgetReason::HomeMismatch, false),
              (ForgetReason::FallbackOnly, false),
              (ForgetReason::InProgress, true),
              (ForgetReason::IoError, true),
              (ForgetReason::TimedOut, true),
              (ForgetReason::StillPresent, true),
          ] {
              assert_eq!(retryable(reason), retried, "{reason:?}");
          }
      }

      #[test]
      fn the_version_is_codex_clis_own_line() {
          assert_eq!(parse_version("codex-cli 0.155.1\n"), Some("0.155.1"));
          assert_eq!(parse_version("codex-cli 0.155.1"), Some("0.155.1"));
          for bad in [
              "",
              "codex 0.155.1",
              "codex-cli 0.155",
              "codex-cli 0.155.1-alpha",
              "codex-cli  \n",
          ] {
              assert_eq!(parse_version(bad), None, "{bad:?}");
          }
      }

      /// B5: the anchored match, built from `recorder.rs`'s naming.
      #[test]
      fn only_the_threads_own_rollout_names_match() {
          let rollout = "019a0b1c-2d3e-7f40-8a5b-111111111111";
          for good in [
              format!("rollout-2026-10-02T10-00-00-{ID}.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{ID}.jsonl.zst"),
              format!("rollout-2026-10-02T10-00-00-{ID}_{rollout}.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{ID}_{rollout}.jsonl.zst"),
          ] {
              assert!(is_rollout_of(good.as_bytes(), ID), "{good}");
          }
          for bad in [
              // Another thread, and another thread whose rollout id is `ID`.
              format!("rollout-2026-10-02T10-00-00-{rollout}.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{rollout}_{ID}.jsonl"),
              // A prefix, a suffix, another extension.
              format!("xrollout-2026-10-02T10-00-00-{ID}.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{ID}f.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{ID}.jsonl.bak"),
              format!("rollout-2026-10-02T10-00-00-{ID}.json"),
              format!("rollout-2026-10-02T10-00-00-{ID}_.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{ID}_{rollout}x.jsonl"),
              format!("rollout-2026-10-02T10-00-00-{ID}"),
              // A timestamp out of shape.
              format!("rollout-2026-10-02 10-00-00-{ID}.jsonl"),
              format!("rollout-2026-10-0210-00-00-{ID}.jsonl"),
              format!("rollout-{ID}.jsonl"),
              // A path, not a name.
              format!("x/rollout-2026-10-02T10-00-00-{ID}.jsonl"),
          ] {
              assert!(!is_rollout_of(bad.as_bytes(), ID), "{bad}");
          }
          // An id that is not one never matches, even itself.
          let name = "rollout-2026-10-02T10-00-00-../../x.jsonl";
          assert!(!is_rollout_of(name.as_bytes(), "../../x"));
      }

      /// R2: a date directory on another file system is not entered, and
      /// counted (`dev ^ 1` stands for another device, as 9d-i's tests do).
      #[test]
      fn a_date_directory_on_another_file_system_is_not_entered() {
          let dir = tempfile::tempdir().unwrap();
          let day = dir.path().join("sessions/2026/10/02");
          std::fs::create_dir_all(&day).unwrap();
          let rollout = day.join(format!("rollout-2026-10-02T10-00-00-{ID}.jsonl"));
          std::fs::write(&rollout, "x").unwrap();
          let sessions = walk::open_root(&dir.path().join("sessions")).unwrap();
          let dev = walk::stat_fd(sessions.as_raw_fd()).unwrap().st_dev;
          let hooks = walk::Hooks::default();
          let mut found = 0;
          for (dev, want) in [(dev ^ 1, 0), (dev, 1)] {
              let walker = Walk {
                  id: ID,
                  dev,
                  hooks: &hooks,
                  max: SESSIONS_DEPTH,
              };
              let mut tally = Tally::default();
              found = 0;
              walker.dir(
                  sessions.as_raw_fd(),
                  Path::new("sessions"),
                  0,
                  &mut |_, _, _, _| found += 1,
                  &mut tally,
              );
              assert_eq!(found, want);
              let left = tally.into_forgotten().remaining;
              assert_eq!(
                  left.iter().any(|r| r.reason == ForgetReason::MountPoint),
                  want == 0,
                  "{left:?}"
              );
          }
          assert_eq!(found, 1);
      }

      #[test]
      fn date_directories_are_codexs_own_shape() {
          assert!(is_date_dir(b"2026", 1) && is_date_dir(b"10", 2) && is_date_dir(b"02", 3));
          for (name, depth) in [(&b"26"[..], 1), (b"2026", 2), (b"1", 2), (b"0a", 3), (b"..", 2)] {
              assert!(!is_date_dir(name, depth), "{name:?} at {depth}");
          }
      }
  }
  ```

  In `crates/hennery-host/src/lib.rs`, replace:

  ```rust
  pub mod forget;
  pub mod git;
  ```

  with:

  ```rust
  pub mod forget;
  pub mod forget_codex;
  pub mod git;
  ```

  In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
      pub profiles: HashMap<String, Profile>,
      /// One line per agent left out, or launched with a caveat.
  ```

  with:

  ```rust
      pub profiles: HashMap<String, Profile>,
      /// The Codex CLI a forget runs `app-server` from (plan 9d decision 9):
      /// `<node> <set>/codex/<the pinned launcher>` when the set has it, or
      /// `--use-cli codex=…`'s binary. Its version is checked when it runs.
      pub codex_app_server: Option<AgentCommand>,
      /// One line per agent left out, or launched with a caveat.
  ```

  In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
                      own_cli = true;
                      command.env.push((var.to_string(), path.to_string_lossy().into_owned()));
  ```

  with:

  ```rust
                      own_cli = true;
                      if name == crate::agent_home::CODEX {
                          out.codex_app_server = Some(AgentCommand {
                              program: path.to_string_lossy().into_owned(),
                              args: Vec::new(),
                              env: Vec::new(),
                          });
                      }
                      command.env.push((var.to_string(), path.to_string_lossy().into_owned()));
  ```

  In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
              (None, _) => {}
  ```

  with:

  ```rust
              (None, _) => {
                  if name == crate::agent_home::CODEX {
                      out.codex_app_server = bundled_codex(set);
                  }
              }
  ```

  In `crates/hennery-host/src/runtime/agents.rs`, replace:

  ```rust
      out
  }
  ```

  with:

  ```rust
      out
  }

  /// The set's bundled Codex launcher, as the embedded manifest pins it
  /// (`codex_app_server.bin`), run by the set's Node: if the set has it.
  fn bundled_codex(set: &InstalledSet) -> Option<AgentCommand> {
      let pin = super::manifest::Manifest::embedded().codex_app_server?;
      let launcher = set.path.join(crate::agent_home::CODEX).join(&pin.bin);
      launcher.is_file().then(|| AgentCommand {
          program: set.node.to_string_lossy().into_owned(),
          args: vec![launcher.to_string_lossy().into_owned()],
          env: Vec::new(),
      })
  }
  ```

  In `crates/hennery-host/src/runtime/manifest.rs`, replace:

  ```rust
      pub adapters: BTreeMap<String, Adapter>,
  }
  ```

  with:

  ```rust
      pub adapters: BTreeMap<String, Adapter>,
      /// How a forget calls the bundled Codex's `app-server` (plan 9d
      /// decision 9): pinned for one Codex version.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub codex_app_server: Option<CodexAppServer>,
  }

  /// The placeholder `params_shape` holds where the thread id goes.
  pub const THREAD_ID_PLACEHOLDER: &str = "{thread_id}";

  /// The call shape of Codex's `thread/delete`, pinned for one Codex version
  /// (plan 9d decision 9). A forget runs the app-server only when its
  /// `--version` is `codex_version`.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct CodexAppServer {
      /// The Codex version (`@openai/codex` in the codex adapter's lockfile)
      /// this shape was read from, e.g. `0.155.1`.
      pub codex_version: String,
      /// The bundled CLI's launcher, relative to the codex adapter's directory
      /// in a set; run as `<node> <bin> app-server`, as codex-acp runs it.
      pub bin: String,
      /// `initialize`'s params, sent as they are.
      pub initialize: AppServerInitialize,
      /// `thread/delete`.
      pub delete_method: String,
      /// The delete's params: one key whose value is `{thread_id}`.
      pub params_shape: BTreeMap<String, String>,
  }

  /// `initialize`'s params (Codex's `v1::InitializeParams`): the client's name
  /// only. Codex's `initialize` has no protocol version; the pinned Codex
  /// version stands for it.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(deny_unknown_fields, rename_all = "camelCase")]
  pub struct AppServerInitialize {
      pub client_info: AppServerClientInfo,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct AppServerClientInfo {
      pub name: String,
      pub title: String,
      pub version: String,
  }

  impl CodexAppServer {
      /// Every rule the shape must keep.
      pub fn validate(&self) -> Result<()> {
          if !is_version(&self.codex_version) {
              bail!("codex_app_server: Codex version {:?} is not x.y.z", self.codex_version);
          }
          check_relative_path(&self.bin).context("codex_app_server: bin")?;
          if !self.bin.starts_with("node_modules/") {
              bail!("codex_app_server: bin {:?} is not under node_modules/", self.bin);
          }
          if self.delete_method != "thread/delete" {
              bail!(
                  "codex_app_server: delete_method {:?} is not thread/delete",
                  self.delete_method
              );
          }
          let placeholders = self
              .params_shape
              .values()
              .filter(|v| v.as_str() == THREAD_ID_PLACEHOLDER)
              .count();
          if self.params_shape.len() != 1 || placeholders != 1 {
              bail!("codex_app_server: params_shape is not one key holding {THREAD_ID_PLACEHOLDER}");
          }
          let info = &self.initialize.client_info;
          if [&info.name, &info.title, &info.version].iter().any(|s| s.is_empty()) {
              bail!("codex_app_server: an empty clientInfo field");
          }
          Ok(())
      }

      /// The delete's params for `thread_id`.
      pub fn params(&self, thread_id: &str) -> serde_json::Map<String, serde_json::Value> {
          self.params_shape
              .iter()
              .map(|(key, value)| {
                  let value = if value == THREAD_ID_PLACEHOLDER {
                      thread_id.to_string()
                  } else {
                      value.clone()
                  };
                  (key.clone(), serde_json::Value::String(value))
              })
              .collect()
      }
  }
  ```

  In `crates/hennery-host/src/runtime/manifest.rs`, replace:

  ```rust
              bail!("the manifest pins no adapter");
          }
  ```

  with:

  ```rust
              bail!("the manifest pins no adapter");
          }
          if let Some(app_server) = &self.codex_app_server {
              if !self.adapters.contains_key(crate::agent_home::CODEX) {
                  bail!("codex_app_server is pinned, but no codex adapter is");
              }
              app_server.validate()?;
          }
  ```

  In `crates/hennery-host/src/runtime/manifest.rs`, replace:

  ```rust
      }

      #[test]
      fn unknown_fields_are_refused() {
  ```

  with:

  ```rust
      }

      /// Plan 9d decision 9: the embedded manifest pins `thread/delete`'s call
      /// shape for the Codex its codex adapter bundles.
      #[test]
      fn the_embedded_manifest_pins_codexs_delete_for_its_bundled_version() {
          let manifest = Manifest::embedded();
          let pin = manifest.codex_app_server.as_ref().expect("pinned");
          assert_eq!(pin.codex_version, "0.155.1");
          assert_eq!(pin.bin, "node_modules/@openai/codex/bin/codex.js");
          assert_eq!(pin.delete_method, "thread/delete");
          assert_eq!(
              serde_json::Value::Object(pin.params("0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3")),
              serde_json::json!({ "threadId": "0b9c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3" })
          );
          assert_eq!(
              serde_json::to_value(&pin.initialize).unwrap(),
              serde_json::json!({ "clientInfo": { "name": "hennery", "title": "hennery", "version": "1" } })
          );
          // The bundled launcher is a file of the pinned `@openai/codex`.
          for platform in Platform::ALL {
              let files = &manifest.adapters["codex"].platforms[platform.key()];
              assert!(
                  files
                      .iter()
                      .any(|f| pin.bin.starts_with(&format!("{}/", f.path)) && f.path == "node_modules/@openai/codex")
              );
          }
      }

      #[test]
      fn a_codex_app_server_pin_out_of_shape_is_refused() {
          let good = Manifest::embedded();
          let pin = || good.codex_app_server.clone().unwrap();
          let mut cases: Vec<(&str, CodexAppServer)> = Vec::new();
          let mut version = pin();
          version.codex_version = "0.155".into();
          cases.push(("x.y.z", version));
          let mut bin = pin();
          bin.bin = "../codex".into();
          cases.push(("plain relative", bin));
          let mut outside = pin();
          outside.bin = "bin/codex".into();
          cases.push(("under node_modules", outside));
          let mut method = pin();
          method.delete_method = "thread/archive".into();
          cases.push(("thread/delete", method));
          let mut shape = pin();
          shape.params_shape.insert("extra".into(), "x".into());
          cases.push(("one key", shape));
          let mut no_placeholder = pin();
          no_placeholder.params_shape = BTreeMap::from([("threadId".into(), "fixed".into())]);
          cases.push(("one key", no_placeholder));
          let mut empty = pin();
          empty.initialize.client_info.name.clear();
          cases.push(("empty clientInfo", empty));
          for (want, case) in cases {
              let mut manifest = good.clone();
              manifest.codex_app_server = Some(case);
              let err = format!("{:#}", manifest.validate().unwrap_err());
              assert!(err.contains(want), "{want}: {err}");
          }
          let mut no_codex = good.clone();
          no_codex.adapters.remove("codex");
          let err = format!("{:#}", no_codex.validate().unwrap_err());
          assert!(err.contains("no codex adapter"), "{err}");
      }

      #[test]
      fn unknown_fields_are_refused() {
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
      fn listed(&self, _path: &Path, _entries: &[CString]) {
  ```

  with:

  ```rust
      pub(crate) fn listed(&self, _path: &Path, _entries: &[CString]) {
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
      fn stated(&self, _path: &Path) {
  ```

  with:

  ```rust
      pub(crate) fn stated(&self, _path: &Path) {
  ```

  In `crates/hennery-host/src/walk.rs`, replace:

  ```rust
              e => return Err(e),
          }
      }
  }

  /// The entries of the open directory `dir`, `.` and `..` left out, read
  ```

  with:

  ```rust
              e => return Err(e),
          }
      }
  }

  /// Unlink the entry `name` in `dir`, never a directory (`unlinkat(…, 0)`):
  /// a regular file the caller found with `stat_at`, or, if one was swapped
  /// in since, the symlink itself, never its target. Gone already is fine.
  pub fn unlink_file_at(dir: RawFd, name: &CStr) -> Result<(), i32> {
      unlink_at(dir, name, 0)
  }

  pub fn is_file(st: &libc::stat) -> bool {
      st.st_mode & libc::S_IFMT == libc::S_IFREG
  }

  /// The entries of the open directory `dir`, `.` and `..` left out, read
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      self, Adapter, File, Manifest, NPM_REGISTRY, Node, NodeArchive, Platform, SCHEMA,
  ```

  with:

  ```rust
      self, Adapter, CodexAppServer, File, Manifest, NPM_REGISTRY, Node, NodeArchive, Platform, SCHEMA,
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      pub adapters: BTreeMap<String, AdapterPin>,
  }
  ```

  with:

  ```rust
      pub adapters: BTreeMap<String, AdapterPin>,
      /// `thread/delete`'s call shape for the bundled Codex (plan 9d decision
      /// 9), carried into the manifest as it is.
      #[serde(default)]
      pub codex_app_server: Option<CodexAppServer>,
  }
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
                  bail!("{name}: no CLI package prefix");
              }
          }
          Ok(pins)
  ```

  with:

  ```rust
                  bail!("{name}: no CLI package prefix");
              }
          }
          if let Some(app_server) = &pins.codex_app_server {
              app_server.validate()?;
          }
          Ok(pins)
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      }
      let manifest = Manifest {
  ```

  with:

  ```rust
      }
      if let Some(pin) = &pins.codex_app_server {
          let lock = locks
              .get(hennery_host::agent_home::CODEX)
              .context("codex_app_server is pinned, but no codex adapter is")?;
          check_codex_app_server(pin, lock)?;
      }
      let manifest = Manifest {
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      };
      manifest.validate()?;
      Ok(manifest)
  ```

  with:

  ```rust
          codex_app_server: pins.codex_app_server.clone(),
      };
      manifest.validate()?;
      Ok(manifest)
  }

  /// The app-server's call shape is read from one Codex version (plan 9d
  /// decision 9): its launcher must be a file of a package the codex lockfile
  /// installs, and that package must be at the pinned version.
  fn check_codex_app_server(pin: &CodexAppServer, lock: &Lock) -> Result<()> {
      let (path, entry) = lock
          .packages
          .iter()
          .filter(|(path, _)| !path.is_empty() && pin.bin.starts_with(&format!("{path}/")))
          .max_by_key(|(path, _)| path.len())
          .with_context(|| {
              format!(
                  "codex_app_server: bin {:?} is not in a package of the codex lockfile",
                  pin.bin
              )
          })?;
      let name = package_name(path, entry)?;
      let version = entry
          .version
          .as_deref()
          .with_context(|| format!("{path}: no version"))?;
      if version != pin.codex_version {
          bail!(
              "codex_app_server pins Codex {}, but the codex lockfile bundles {name} {version}: \
               read thread/delete's call shape from that version and update the pin",
              pin.codex_version
          );
      }
      Ok(())
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      }

      #[test]
      fn npms_platform_lists_allow_negation() {
  ```

  with:

  ```rust
      }

      /// `PINS`, with a codex adapter bundling `@vendor/codex` and the
      /// app-server's call shape pinned for its version (plan 9d decision 9).
      const CODEX_PINS: &str = r#"
  [adapters.codex]
  package = "@acp/codex"
  version = "1.0.0"
  entry = "dist/index.js"
  cli = ["node_modules/@vendor/codex-"]
  [codex_app_server]
  codex_version = "0.155.1"
  bin = "node_modules/@vendor/codex/bin/codex.js"
  delete_method = "thread/delete"
  params_shape = { threadId = "{thread_id}" }
  [codex_app_server.initialize.clientInfo]
  name = "hennery"
  title = "hennery"
  version = "1"
  "#;

      /// The codex adapter's lockfile and registry for `CODEX_PINS`, its
      /// `@vendor/codex` at `codex_version`.
      fn codex_fixture(codex_version: &str) -> (Pins, BTreeMap<String, Lock>, Registry) {
          let pins = Pins::parse(&format!("{PINS}{CODEX_PINS}")).unwrap();
          let claude = Fixture::new();
          let mut codex = Fixture {
              lock: Lock {
                  lockfile_version: 3,
                  packages: BTreeMap::from([(
                      String::new(),
                      LockEntry {
                          dependencies: BTreeMap::from([("@acp/codex".into(), "1.0.0".into())]),
                          ..LockEntry::default()
                      },
                  )]),
              },
              registry: claude.registry.clone(),
          };
          codex.add("node_modules/@acp/codex", "@acp/codex", "1.0.0", &[], &[], &[], false);
          codex.add(
              "node_modules/@vendor/codex",
              "@vendor/codex",
              codex_version,
              &[],
              &[],
              &[],
              false,
          );
          for (suffix, os, cpu) in [
              ("linux-x64", "linux", "x64"),
              ("linux-arm64", "linux", "arm64"),
              ("darwin-arm64", "darwin", "arm64"),
          ] {
              codex.add(
                  &format!("node_modules/@vendor/codex-{suffix}"),
                  &format!("@vendor/codex-{suffix}"),
                  codex_version,
                  &[os],
                  &[cpu],
                  &[],
                  true,
              );
          }
          let locks = BTreeMap::from([("claude".into(), claude.lock), ("codex".into(), codex.lock)]);
          (pins, locks, codex.registry)
      }

      fn node_archives() -> BTreeMap<Platform, NodeArchive> {
          Platform::ALL
              .into_iter()
              .map(|p| {
                  (
                      p,
                      NodeArchive {
                          url: format!("https://nodejs.org/dist/v24.21.0/node-v24.21.0-{}.tar.gz", p.key()),
                          sha256: "a".repeat(64),
                          archive_size: 10,
                          node_size: 20,
                      },
                  )
              })
              .collect()
      }

      /// Plan 9d decision 9: the app-server's call shape is carried into the
      /// manifest, and only for the Codex version the codex lockfile bundles.
      #[test]
      fn the_codex_app_server_pin_is_carried_only_for_the_bundled_codex_version() {
          let (pins, locks, registry) = codex_fixture("0.155.1");
          let manifest = build(&pins, &locks, &registry, &node_archives()).unwrap();
          let pin = manifest.codex_app_server.expect("carried");
          assert_eq!(pin.codex_version, "0.155.1");
          assert_eq!(pin.bin, "node_modules/@vendor/codex/bin/codex.js");
          let (pins, locks, registry) = codex_fixture("0.156.0");
          let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
          assert!(err.contains("bundles @vendor/codex 0.156.0"), "{err}");
          // A launcher outside every package of the codex lockfile.
          let (mut pins, locks, registry) = codex_fixture("0.155.1");
          pins.codex_app_server.as_mut().unwrap().bin = "node_modules/@other/codex/bin/codex.js".into();
          let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
          assert!(err.contains("not in a package of the codex lockfile"), "{err}");
          // A shape out of its rules is refused as pins.toml is read.
          let bad = format!("{PINS}{CODEX_PINS}").replace("\"thread/delete\"", "\"thread/archive\"");
          assert!(format!("{:#}", Pins::parse(&bad).unwrap_err()).contains("thread/delete"));
          // And none without a pin.
          let (mut pins, locks, registry) = codex_fixture("0.155.1");
          pins.codex_app_server = None;
          assert!(
              build(&pins, &locks, &registry, &node_archives())
                  .unwrap()
                  .codex_app_server
                  .is_none()
          );
      }

      #[test]
      fn npms_platform_lists_allow_negation() {
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      /// The transcript and its family in each project directory (B9).
  ```

  with:

  ```rust
      /// The transcript and its family in each project directory (B9); for
      /// Codex, its rollout files in `sessions/` and `archived_sessions/`
      /// (plan 9d-ii).
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
              Self::Transcript => "projects/*/<id>.jsonl (and its family)",
  ```

  with:

  ```rust
              Self::Transcript => "projects/*/<id>.jsonl (and its family), or sessions/**/rollout-*-<id>.jsonl",
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      IoError,
      /// The collector's own: the host answered `error{invalid}` for the id
  ```

  with:

  ```rust
      IoError,
      /// Codex's `thread/delete` refused: forked history in another thread
      /// still references the rollout (plan 9d-ii, decision 9). Final, and
      /// never followed by the fallback (B5).
      ForkedHistory,
      /// Codex's `thread/delete` refused: the thread was never persisted
      /// (plan 9d-ii, decision 9). Final.
      Ephemeral,
      /// The app-server named another `CODEX_HOME` than the session's
      /// recorded one in its `initialize` answer: nothing was asked of it, and
      /// no fallback ran (plan 9d-ii, the parent's rule).
      HomeMismatch,
      /// Only the fallback ran (plan 9d-ii, decision 10): Codex's own database
      /// may still hold copies of the conversation. Final.
      FallbackOnly,
      /// The collector's own: the host answered `error{invalid}` for the id
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
  use hennery_proto::frames::{CollectorFrame, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame};
  ```

  with:

  ```rust
  use hennery_proto::frames::{
      CollectorFrame, ForgetKind, ForgetOutcome, ForgetReason, ForgetRemaining, ForgetWhat, HostFrame,
  };
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust

  /// The notes a session of `agent` always gets (plan 9d decision 7).
  ```

  with:

  ```rust

  /// What is never removed for a Codex session, whatever the outcome (plan
  /// 9d-ii, O12). Always in a Codex session's result.
  pub const CODEX_NOTES: [&str; 1] = ["the agent's history.jsonl and logs_2.sqlite may still name this session"];

  /// What the fallback leaves (plan 9d decision 10, B5): in a Codex result
  /// whose remaining items name `codex_database_copies`.
  pub const CODEX_FALLBACK_NOTES: [&str; 2] = [
      "conversation copies may remain in Codex's own database",
      "rollouts of the session's subagent threads are out of the fallback's reach",
  ];

  /// The notes a session of `agent` always gets (plan 9d decision 7).
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
          _ => Vec::new(),
      }
  ```

  with:

  ```rust
          "codex" => CODEX_NOTES.iter().map(|n| n.to_string()).collect(),
          _ => Vec::new(),
      }
  }

  /// `notes`, and what a result's remaining items add: the fallback's for
  /// `codex_database_copies` (plan 9d-ii).
  pub fn notes_for(agent: &str, remaining: &[RemovalItem]) -> Vec<String> {
      let mut out = notes(agent);
      if remaining.iter().any(|r| r.kind == ForgetKind::CodexDatabaseCopies) {
          out.extend(CODEX_FALLBACK_NOTES.iter().map(|n| n.to_string()));
      }
      out
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
  pub fn combined(had_agent_record: bool, agent: &str, results: Vec<TranscriptRemoval>) -> TranscriptRemoval {
      let notes = notes(agent);
      if !had_agent_record {
  ```

  with:

  ```rust
  pub fn combined(had_agent_record: bool, agent: &str, results: Vec<TranscriptRemoval>) -> TranscriptRemoval {
      if !had_agent_record {
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
              notes,
  ```

  with:

  ```rust
              notes: notes(agent),
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
      TranscriptRemoval {
          state,
          pending: results.iter().find_map(|r| r.pending),
          remaining: results.into_iter().flat_map(|r| r.remaining).collect(),
          notes,
  ```

  with:

  ```rust
      let pending = results.iter().find_map(|r| r.pending);
      let remaining: Vec<RemovalItem> = results.into_iter().flat_map(|r| r.remaining).collect();
      TranscriptRemoval {
          state,
          pending,
          notes: notes_for(agent, &remaining),
          remaining,
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
      let notes = notes(&record.agent);
  ```

  with:

  ```rust
      let agent = record.agent.clone();
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
              result.notes = notes;
  ```

  with:

  ```rust
              result.notes = notes_for(&agent, &result.remaining);
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
          assert!(mixed.notes.is_empty());
  ```

  with:

  ```rust
          assert_eq!(mixed.notes, CODEX_NOTES);
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
              (RemovalState::Pending, Some(RemovalPending::HostOffline), 1)
          );
      }

  ```

  with:

  ```rust
              (RemovalState::Pending, Some(RemovalPending::HostOffline), 1)
          );
      }

      /// Plan 9d-ii, O12, decision 10: a Codex session always names Codex's
      /// other residue; one the fallback handled also says its database
      /// copies may remain and that subagent rollouts are out of its reach.
      /// Any other agent gets none.
      #[test]
      fn codex_notes_name_its_residue_and_what_the_fallback_leaves() {
          let removed = answered(ForgetOutcome::Complete, &[]).0;
          let deleted = combined(true, "codex", vec![removed.clone()]);
          assert_eq!(deleted.notes, CODEX_NOTES);
          assert!(deleted.notes[0].contains("history.jsonl") && deleted.notes[0].contains("logs_2.sqlite"));
          let copies = left(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false);
          let (fell_back, done) = answered(ForgetOutcome::Partial, &[copies]);
          assert_eq!((fell_back.state, done), (RemovalState::Partial, true));
          let fell_back = combined(true, "codex", vec![fell_back]);
          let mut want: Vec<&str> = CODEX_NOTES.to_vec();
          want.extend(CODEX_FALLBACK_NOTES);
          assert_eq!(fell_back.notes, want);
          assert!(
              fell_back
                  .notes
                  .iter()
                  .any(|n| n == "conversation copies may remain in Codex's own database")
          );
          assert!(fell_back.notes.iter().any(|n| n.contains("subagent")));
          assert!(combined(true, "gemini", vec![removed]).notes.is_empty());
      }

  ```

  In `crates/hennery-testkit/Cargo.toml`, replace:

  ```toml
  path = "src/bin/hennery-fake-acp.rs"

  ```

  with:

  ```toml
  path = "src/bin/hennery-fake-acp.rs"

  # Codex's CLI as a forget runs it: `--version` and `app-server` (plan 9d-ii).
  [[bin]]
  name = "hennery-fake-codex"
  path = "src/bin/hennery-fake-codex.rs"

  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
                          let deleted = delete_session(&script, &req.session_id);
  ```

  with:

  ```rust
                          let deleted = if script.codex_archive_log.is_some() {
                              codex_archive(&script, &req.session_id)
                          } else {
                              delete_session(&script, &req.session_id)
                          };
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
      Err(format!("Session {id} not found in any project directory"))
  }
  ```

  with:

  ```rust
      Err(format!("Session {id} not found in any project directory"))
  }

  /// `session/delete` as codex-acp 1.13.0 runs it
  /// (`FakeScript::codex_archive_log`): Codex's `thread/archive`, which moves
  /// each of the session's rollouts under `$CODEX_HOME/sessions/` (at most
  /// three levels down) to `$CODEX_HOME/archived_sessions/`, keeping its name.
  /// None is "no rollout found", as Codex 0.155.1 answers.
  fn codex_archive(script: &FakeScript, session: &SessionId) -> Result<(), String> {
      let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "-".into());
      if let Some(log) = &script.codex_archive_log {
          let cwd = std::env::current_dir()
              .map(|d| d.display().to_string())
              .unwrap_or_default();
          let mut file = std::fs::OpenOptions::new()
              .create(true)
              .append(true)
              .open(log)
              .expect("open codex_archive_log");
          writeln!(
              file,
              "CODEX_HOME={}\ncwd={cwd}\nCODEX_SQLITE_HOME={}",
              var("CODEX_HOME"),
              var("CODEX_SQLITE_HOME"),
          )
          .expect("write codex_archive_log");
      }
      let Some(home) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) else {
          return Err("no CODEX_HOME".into());
      };
      let home = std::path::Path::new(&home);
      let id = session.to_string();
      fn walk(dir: &std::path::Path, depth: usize, id: &str, out: &mut Vec<std::path::PathBuf>) {
          for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
              let path = entry.path();
              let name = entry.file_name().to_string_lossy().into_owned();
              match std::fs::symlink_metadata(&path) {
                  Ok(meta) if meta.is_file() && hennery_testkit::names_rollout_of(&name, id) => out.push(path),
                  Ok(meta) if meta.is_dir() && depth < 3 && hennery_testkit::is_codex_date_dir(&name, depth + 1) => {
                      walk(&path, depth + 1, id, out)
                  }
                  _ => {}
              }
          }
      }
      let mut found = Vec::new();
      walk(&home.join("sessions"), 0, &id, &mut found);
      if found.is_empty() {
          return Err(format!("no rollout found for thread id {id}"));
      }
      let archived = home.join("archived_sessions");
      std::fs::create_dir_all(&archived).map_err(|e| e.to_string())?;
      for path in found {
          let name = path.file_name().expect("a rollout's name").to_owned();
          std::fs::rename(&path, archived.join(name)).map_err(|e| e.to_string())?;
      }
      Ok(())
  }
  ```

  Create `crates/hennery-testkit/src/bin/hennery-fake-codex.rs`:

  ```rust
  //! A stand-in for Codex 0.155.1's CLI as a forget runs it (plan 9d-ii,
  //! decision 9): `--version`, and `app-server` speaking its JSON-RPC over
  //! stdio, one JSON object per line with no `jsonrpc` field, answering as
  //! 0.155.1 does (read at `rust-v0.155.1`, and seen from a live run of it):
  //! - `initialize` → `{userAgent, codexHome, platformFamily, platformOs}`,
  //!   then a `remoteControl/status/changed` notification nobody asked for;
  //! - any request before it → `-32600 Not initialized`;
  //! - a method it does not know → `-32600 Invalid request: unknown variant`;
  //! - `thread/delete` → as `FakeCodex::delete` says.
  //!
  //! Every spawn and every line read is logged (`FakeCodex::log`), so a test
  //! sees the exact frames the host sent, and whether it ran at all.

  use hennery_testkit::{CODEX_SCRIPT_ENV, FakeCodex, FakeDelete, FakeInitialize};
  use serde_json::{Value, json};
  use std::io::{BufRead, Write};
  use std::path::{Path, PathBuf};

  fn main() {
      let script: FakeCodex = std::env::var(CODEX_SCRIPT_ENV)
          .ok()
          .map(|s| serde_json::from_str(&s).expect("valid fake codex script JSON"))
          .unwrap_or_default();
      let args: Vec<String> = std::env::args().skip(1).collect();
      let var = |name: &str| std::env::var(name).unwrap_or_else(|_| "-".into());
      log(
          &script,
          json!({ "spawn": {
              "args": args,
              "CODEX_HOME": var("CODEX_HOME"),
              "CODEX_SQLITE_HOME": var("CODEX_SQLITE_HOME"),
              "CLAUDE_CODE_PROJECT_DIR_NAME": var("CLAUDE_CODE_PROJECT_DIR_NAME"),
              "cwd": std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default(),
          }}),
      );
      match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
          ["--version"] if script.hang_version => {
              if let Some(path) = &script.pid_file {
                  std::fs::write(path, std::process::id().to_string()).expect("write the pid file");
              }
              hang()
          }
          ["--version"] => println!("codex-cli {}", script.version),
          ["app-server"] => app_server(&script),
          other => {
              eprintln!("hennery-fake-codex: unexpected arguments {other:?}");
              std::process::exit(2);
          }
      }
  }

  fn log(script: &FakeCodex, line: Value) {
      if script.log.is_empty() {
          return;
      }
      let mut file = std::fs::OpenOptions::new()
          .create(true)
          .append(true)
          .open(&script.log)
          .expect("open the fake codex log");
      writeln!(file, "{line}").expect("write the fake codex log");
  }

  fn send(out: &mut impl Write, value: Value) {
      writeln!(out, "{value}").expect("write to the host");
      out.flush().expect("flush to the host");
  }

  fn error(id: &Value, code: i64, message: impl Into<String>) -> Value {
      json!({ "error": { "code": code, "message": message.into() }, "id": id })
  }

  fn hang() -> ! {
      loop {
          std::thread::sleep(std::time::Duration::from_secs(600));
      }
  }

  fn app_server(script: &FakeCodex) {
      if let Some(path) = &script.pid_file {
          std::fs::write(path, std::process::id().to_string()).expect("write the pid file");
      }
      if script.ignore_term {
          // SAFETY: signal(2) with SIG_IGN, before any other thread exists.
          unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
      }
      let stdin = std::io::stdin();
      let mut out = std::io::stdout();
      let mut initialized = false;
      for line in stdin.lock().lines() {
          let Ok(line) = line else { return };
          log(script, json!({ "recv": line }));
          let Ok(message) = serde_json::from_str::<Value>(&line) else {
              continue;
          };
          let (Some(id), Some(method)) = (
              message.get("id").cloned(),
              message.get("method").and_then(Value::as_str),
          ) else {
              // A notification (`initialized`, say) or an answer: only logged.
              continue;
          };
          match method {
              "initialize" => match script.initialize {
                  FakeInitialize::Answer => {
                      initialized = true;
                      let home = script.codex_home.clone().unwrap_or_else(|| {
                          let home = std::env::var("CODEX_HOME").unwrap_or_default();
                          std::fs::canonicalize(&home)
                              .map(|p| p.display().to_string())
                              .unwrap_or(home)
                      });
                      send(
                          &mut out,
                          json!({ "id": id, "result": {
                              "userAgent": "hennery/0.155.1 (fake) unknown (hennery; 1)",
                              "codexHome": home,
                              "platformFamily": "unix",
                              "platformOs": std::env::consts::OS,
                          }}),
                      );
                      send(
                          &mut out,
                          json!({ "method": "remoteControl/status/changed",
                                  "params": { "status": "disabled" }, "emittedAtMs": 0 }),
                      );
                  }
                  FakeInitialize::Error => send(
                      &mut out,
                      error(
                          &id,
                          -32600,
                          "Invalid clientInfo.name: ''. Must be a valid HTTP header value.",
                      ),
                  ),
                  FakeInitialize::Hang => hang(),
                  FakeInitialize::Exit => std::process::exit(1),
              },
              _ if !initialized => send(&mut out, error(&id, -32600, "Not initialized")),
              "thread/delete" if script.delete != FakeDelete::UnknownMethod => {
                  let thread = message["params"]["threadId"].as_str().unwrap_or_default().to_string();
                  delete(script, &mut out, &id, &thread);
              }
              other => send(
                  &mut out,
                  error(
                      &id,
                      -32600,
                      format!("Invalid request: unknown variant `{other}`, expected one of `initialize`, `thread/start`"),
                  ),
              ),
          }
      }
  }

  fn delete(script: &FakeCodex, out: &mut impl Write, id: &Value, thread: &str) {
      match script.delete {
          FakeDelete::Delete => {
              let home = PathBuf::from(std::env::var("CODEX_HOME").unwrap_or_default());
              let found = rollouts(&home, thread);
              if found.is_empty() {
                  return send(
                      out,
                      error(id, -32600, format!("no rollout found for thread id {thread}")),
                  );
              }
              for path in found {
                  std::fs::remove_file(path).expect("remove a rollout");
              }
              send(out, json!({ "id": id, "result": {} }));
              send(
                  out,
                  json!({ "method": "thread/deleted", "params": { "threadId": thread }, "emittedAtMs": 0 }),
              );
          }
          FakeDelete::AnswerButKeep => send(out, json!({ "id": id, "result": {} })),
          FakeDelete::ForkedHistory => send(
              out,
              error(
                  id,
                  -32600,
                  format!("cannot delete thread {thread}: forked history still references it"),
              ),
          ),
          FakeDelete::Ephemeral => send(
              out,
              error(
                  id,
                  -32600,
                  format!("thread is not persisted and cannot be deleted: {thread}"),
              ),
          ),
          FakeDelete::LiveWorker => send(
              out,
              error(id, -32600, "live internal threads can only be removed by their owner"),
          ),
          FakeDelete::MethodNotFound => send(out, error(id, -32601, "thread/delete is not supported yet")),
          FakeDelete::Internal => send(
              out,
              error(
                  id,
                  -32603,
                  format!("failed to delete thread: database is locked ({thread})"),
              ),
          ),
          FakeDelete::Hang => hang(),
          FakeDelete::Exit => std::process::exit(1),
          FakeDelete::UnknownMethod => unreachable!("answered as an unknown method"),
      }
  }

  /// The thread's rollouts as Codex finds them: under `sessions/` at most
  /// three levels down, and at the top of `archived_sessions/`; regular files
  /// only, never through a link.
  fn rollouts(home: &Path, thread: &str) -> Vec<PathBuf> {
      fn walk(dir: &Path, depth: usize, thread: &str, out: &mut Vec<PathBuf>) {
          for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
              let path = entry.path();
              let Ok(meta) = std::fs::symlink_metadata(&path) else {
                  continue;
              };
              let name = entry.file_name().to_string_lossy().into_owned();
              if meta.is_file() && hennery_testkit::names_rollout_of(&name, thread) {
                  out.push(path);
              } else if meta.is_dir() && depth < 3 && hennery_testkit::is_codex_date_dir(&name, depth + 1) {
                  walk(&path, depth + 1, thread, out);
              }
          }
      }
      let mut out = Vec::new();
      walk(&home.join("sessions"), 0, thread, &mut out);
      walk(&home.join("archived_sessions"), 3, thread, &mut out);
      out
  }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
      pub delete_waits_for_file: Option<String>,
  }
  ```

  with:

  ```rust
      pub delete_waits_for_file: Option<String>,
      /// On `session/delete`, act as codex-acp 1.13.0 does instead of
      /// Claude's SDK (plan 9d-ii, decision 10): Codex's `thread/archive`,
      /// which moves each rollout of the session under
      /// `$CODEX_HOME/sessions/` (at most three levels down) to
      /// `$CODEX_HOME/archived_sessions/`, deleting nothing. It appends the
      /// environment and cwd it ran with to this file first: `CODEX_HOME`,
      /// `cwd` and `CODEX_SQLITE_HOME` (`-` when unset). With no `CODEX_HOME`
      /// it moves nothing: the fake never touches a real home.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub codex_archive_log: Option<String>,
  }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
              delete_waits_for_file: None,
          }
  ```

  with:

  ```rust
              delete_waits_for_file: None,
              codex_archive_log: None,
          }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
  pub const SCRIPT_ENV: &str = "HENNERY_FAKE_ACP_SCRIPT";

  ```

  with:

  ```rust
  pub const SCRIPT_ENV: &str = "HENNERY_FAKE_ACP_SCRIPT";

  /// Environment variable carrying `hennery-fake-codex`'s script.
  pub const CODEX_SCRIPT_ENV: &str = "HENNERY_FAKE_CODEX_SCRIPT";

  /// Behaviour of `hennery-fake-codex`, a stand-in for Codex 0.155.1's CLI
  /// as a forget runs it (plan 9d-ii, decision 9): `--version`, and
  /// `app-server` speaking its JSON-RPC (one JSON object per line, no
  /// `jsonrpc` field) as 0.155.1 does.
  #[derive(Debug, Clone, Default, Serialize, Deserialize)]
  pub struct FakeCodex {
      /// What `--version` prints after `codex-cli `.
      pub version: String,
      /// Appended to, one JSON object per line: first `{"spawn": …}` (the
      /// arguments, `CODEX_HOME`, `CODEX_SQLITE_HOME`,
      /// `CLAUDE_CODE_PROJECT_DIR_NAME` and the cwd), then `{"recv": line}`
      /// for every line the app-server reads, verbatim.
      pub log: String,
      #[serde(default)]
      pub initialize: FakeInitialize,
      /// `codexHome` in `initialize`'s answer; else `$CODEX_HOME`
      /// canonicalised, as Codex answers.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub codex_home: Option<String>,
      #[serde(default)]
      pub delete: FakeDelete,
      /// The app-server writes its pid here, and ignores SIGTERM if
      /// `ignore_term`: only the group's SIGKILL ends it then.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub pid_file: Option<String>,
      #[serde(default)]
      pub ignore_term: bool,
      /// `--version` never ends (writing `pid_file` first).
      #[serde(default)]
      pub hang_version: bool,
  }

  /// Whether `name` is one of `thread`'s rollout files, as Codex names them
  /// (`rollout-<timestamp>-<thread>[_<rollout>].jsonl[.zst]`): what the fakes
  /// standing in for Codex find, as Codex finds them by its own index. Loose
  /// on the timestamp; the host's own matcher is the strict one.
  pub fn names_rollout_of(name: &str, thread: &str) -> bool {
      let Some(rest) = name.strip_prefix("rollout-").and_then(|r| r.get(20..)) else {
          return false;
      };
      let Some(rest) = rest.strip_prefix(thread) else {
          return false;
      };
      let rest = rest.strip_suffix(".zst").unwrap_or(rest);
      let Some(rest) = rest.strip_suffix(".jsonl") else {
          return false;
      };
      rest.is_empty() || rest.starts_with('_')
  }

  /// Whether `name` is a date directory `depth` levels under `sessions/`
  /// (`YYYY`, then `MM`, then `DD`), as Codex lays them out.
  pub fn is_codex_date_dir(name: &str, depth: usize) -> bool {
      let len = if depth == 1 { 4 } else { 2 };
      name.len() == len && name.bytes().all(|b| b.is_ascii_digit())
  }

  /// How the fake answers `initialize`.
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum FakeInitialize {
      /// As Codex does: `userAgent`, `codexHome`, `platformFamily`,
      /// `platformOs`, then a `remoteControl/status/changed` notification.
      #[default]
      Answer,
      /// `-32600`, as Codex answers a client name it cannot use.
      Error,
      /// Never.
      Hang,
      /// It exits at once, answering nothing.
      Exit,
  }

  /// How the fake answers `thread/delete` (after `initialize`; before it,
  /// `-32600 Not initialized`, as Codex does).
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum FakeDelete {
      /// As Codex does: each rollout of the thread under `sessions/` (at most
      /// three levels down) and `archived_sessions/` is removed, then `{}`
      /// and a `thread/deleted` notification; with none, `-32600 no rollout
      /// found for thread id <id>`.
      #[default]
      Delete,
      /// `{}`, but the rollouts stay: for the check afterwards (B4).
      AnswerButKeep,
      /// `-32600 cannot delete thread <id>: forked history still references it`.
      ForkedHistory,
      /// `-32600 thread is not persisted and cannot be deleted: <id>`.
      Ephemeral,
      /// `-32600 live internal threads can only be removed by their owner`.
      LiveWorker,
      /// The method is unknown to this Codex: `-32600 Invalid request:
      /// unknown variant`, as 0.155.1 answers any method it does not know.
      UnknownMethod,
      /// `-32601`, JSON-RPC's own method not found.
      MethodNotFound,
      /// `-32603 failed to delete thread: …`: a failure midway.
      Internal,
      /// Never.
      Hang,
      /// The app-server exits once it has read the request, answering
      /// nothing: the delete may have run.
      Exit,
  }

  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      let (agents, profiles, _set_in_use) = if args.agents.is_empty() {
          runtime::default_agents(&args.data_dir, &args.mirrors).await
      } else {
          (args.agents.into_iter().collect(), Default::default(), None)
  ```

  with:

  ```rust
      // With `--agent`, no app-server either: a Codex forget takes the
      // fallback.
      let (agents, profiles, codex_app_server, _set_in_use) = if args.agents.is_empty() {
          runtime::default_agents(&args.data_dir, &args.mirrors).await
      } else {
          (args.agents.into_iter().collect(), Default::default(), None, None)
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      cfg.profiles = profiles;
      cfg.idle_timeout = std::time::Duration::from_secs(args.idle_timeout_secs);
  ```

  with:

  ```rust
      cfg.profiles = profiles;
      cfg.codex_app_server = codex_app_server;
      cfg.idle_timeout = std::time::Duration::from_secs(args.idle_timeout_secs);
  ```

  In `crates/hennery/src/runtime.rs`, replace:

  ```rust
  /// installing the pinned one if it is not current, with their profiles.
  /// The file returned holds that set in use for as long as it is kept.
  ```

  with:

  ```rust
  /// installing the pinned one if it is not current, with their profiles and
  /// the Codex CLI a forget runs `app-server` from (plan 9d decision 9). The
  /// file returned holds that set in use for as long as it is kept.
  ```

  In `crates/hennery/src/runtime.rs`, replace:

  ```rust
      HashMap<String, hennery_host::profile::Profile>,
      Option<std::fs::File>,
  ```

  with:

  ```rust
      HashMap<String, hennery_host::profile::Profile>,
      Option<AgentCommand>,
      Option<std::fs::File>,
  ```

  In `crates/hennery/src/runtime.rs`, replace:

  ```rust
      (prepared.agents.agents, prepared.agents.profiles, prepared.in_use)
  ```

  with:

  ```rust
      (
          prepared.agents.agents,
          prepared.agents.profiles,
          prepared.agents.codex_app_server,
          prepared.in_use,
      )
  ```

  In `packaging/README.md`, replace:

  ```markdown

  ## Publishing is the operator's
  ```

  with:

  ```markdown

  ## Pin bumps

  A bump of `adapters/pins.toml` is `hennery-pins lock`, then `generate`,
  then the pin-bump job (`.github/workflows/pins.yml`). These checks are
  live, with logged-in agents, and the operator's to run before it merges:

  - the live e2e gate (umbrella spec §14) on every platform;
  - **Codex's `thread/delete` still removes everything** (plan 9d
    decision 14). After a turn in a scratch `CODEX_HOME`, delete the session
    from hennery and check that nothing of the thread is left: its rollout
    files in `sessions/` and `archived_sessions/`, its rows in
    `thread_history_1.sqlite` (`thread_items`, `thread_turns`,
    `thread_realtime_items`), its rows in the state DB (`state_5.sqlite`),
    and its entries in `session_index.jsonl`. hennery itself only checks the
    rollout files: the rest is `thread/delete`'s contract. If the bundled
    Codex version changed, read `thread/delete`'s call shape from its source
    first and update `[codex_app_server]` in `pins.toml`.

  ## Publishing is the operator's
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
            "description": "The transcript and its family in each project directory (B9).",
  ```

  with:

  ```json
            "description": "The transcript and its family in each project directory (B9); for\nCodex, its rollout files in `sessions/` and `archived_sessions/`\n(plan 9d-ii).",
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
            "description": "The removal failed midway (B3).",
            "type": "string"
  ```

  with:

  ```json
            "description": "The removal failed midway (B3).",
            "type": "string"
          },
          {
            "const": "forked_history",
            "description": "Codex's `thread/delete` refused: forked history in another thread\nstill references the rollout (plan 9d-ii, decision 9). Final, and\nnever followed by the fallback (B5).",
            "type": "string"
          },
          {
            "const": "ephemeral",
            "description": "Codex's `thread/delete` refused: the thread was never persisted\n(plan 9d-ii, decision 9). Final.",
            "type": "string"
          },
          {
            "const": "home_mismatch",
            "description": "The app-server named another `CODEX_HOME` than the session's\nrecorded one in its `initialize` answer: nothing was asked of it, and\nno fallback ran (plan 9d-ii, the parent's rule).",
            "type": "string"
          },
          {
            "const": "fallback_only",
            "description": "Only the fallback ran (plan 9d-ii, decision 10): Codex's own database\nmay still hold copies of the conversation. Final.",
            "type": "string"
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  export type ForgetReason = "attached" | "in_progress" | "unsupported_agent" | "no_recorded_home" | "unknown_to_host" | "shared" | "unsafe_root" | "root_missing" | "symlink" | "not_a_directory" | "unsafe_directory" | "mount_point" | "too_deep" | "timed_out" | "still_present" | "io_error" | "invalid_id" | "host_revoked";
  ```

  with:

  ```typescript
  export type ForgetReason = "attached" | "in_progress" | "unsupported_agent" | "no_recorded_home" | "unknown_to_host" | "shared" | "unsafe_root" | "root_missing" | "symlink" | "not_a_directory" | "unsafe_directory" | "mount_point" | "too_deep" | "timed_out" | "still_present" | "io_error" | "forked_history" | "ephemeral" | "home_mismatch" | "fallback_only" | "invalid_id" | "host_revoked";
  ```


Run: `cargo run -p hennery-proto --bin gen`.

- [ ] **Step 4: Run them to see them pass**

- [ ] **Step 5: Revert-probes**

91 were run, and all are caught. They cover:
- each check before the spawn;
- each environment variable, and the cwd;
- the `codexHome` check;
- each refusal mapping;
- each fallback trigger;
- the rule never to fall back after an answer;
- the anchored rollout name;
- the depth limits;
- the check afterwards;
- the manifest validation, and the `hennery-pins` check.

Three are defence in depth and recorded as such:
- the app-server's terminate call (dropping the process kills the group anyway);
- a symlink swapped in after the check (the second pass reports it);
- a failed write of the delete request.

- [ ] **Step 6: The full checks**

- [ ] **Step 7: Commit**

### Task 2: The review's hardening

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-testkit/tests/forget_codex.rs`, replace:

  ```rust

  /// The process whose pid `pid_file` holds is gone (its group killed).
  ```

  with:

  ```rust

  /// B6 (the review's item 6): the app-server's own child, in its process
  /// group and deaf to SIGTERM, is gone after the forget too: the group is
  /// killed, not only the app-server. Its pid is polled for, never read
  /// straight after the spawn.
  #[tokio::test]
  async fn the_app_servers_grandchild_dies_with_its_group() {
      let root = Root::new();
      root.populate();
      let grandchild = root.base.join("grandchild.pid");
      let mut fake = root.fake(FakeDelete::Hang);
      fake.grandchild_pid_file = Some(grandchild.to_str().unwrap().into());
      let mut ctx = root.ctx(fake);
      // Room for a first exec macOS scans before the app-server starts.
      ctx.deadline = Duration::from_secs(15);
      let run = tokio::spawn(async move {
          root.forget_with(&ctx).await;
          root
      });
      let deadline = Instant::now() + Duration::from_secs(15);
      let pid: i32 = loop {
          if let Some(pid) = std::fs::read_to_string(&grandchild)
              .ok()
              .and_then(|s| s.trim().parse().ok())
          {
              break pid;
          }
          assert!(Instant::now() < deadline, "the grandchild never started");
          tokio::time::sleep(Duration::from_millis(20)).await;
      };
      assert!(
          hennery_testkit::pid_alive(pid),
          "the grandchild runs while the forget waits"
      );
      let _root = tokio::time::timeout(Duration::from_secs(40), run)
          .await
          .expect("the forget keeps its deadline")
          .unwrap();
      assert_gone(&grandchild).await;
  }

  /// The process whose pid `pid_file` holds is gone (its group killed).
  ```

  In `crates/hennery-testkit/tests/forget_codex.rs`, replace:

  ```rust
      assert!(!hennery_testkit::pid_alive(pid), "the app-server outlived its forget");
  }
  ```

  with:

  ```rust
      assert!(!hennery_testkit::pid_alive(pid), "the app-server outlived its forget");
  }

  /// The review's item 4: a fallback whose adapter never answers is cut its
  /// grace before the forget's deadline, so stopping it still ends within
  /// the deadline (and the host's answer inside the collector's wait). Its
  /// archive ran first, so the walk still leaves nothing.
  #[tokio::test]
  async fn a_fallback_adapter_that_never_answers_is_stopped_within_the_deadline() {
      let root = Root::new();
      root.populate();
      let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
      ctx.codex_app_server = None;
      ctx.deadline = Duration::from_secs(6);
      let script = FakeScript {
          codex_archive_log: Some(root.archive_log().to_str().unwrap().into()),
          delete_waits_for_file: Some(root.base.join("never").to_str().unwrap().into()),
          ..FakeScript::default()
      };
      let adapter = ctx.agents.get_mut("codex").unwrap();
      adapter.env.retain(|(k, _)| k != SCRIPT_ENV);
      adapter
          .env
          .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
      let started = Instant::now();
      let forgotten = root.forget_with(&ctx).await;
      // Cut at 4 s (6 s less the 2 s grace), not at 6 s.
      assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
      assert_fell_back(&root, &forgotten, "a hung adapter");
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test forget_codex group_ deadline`
Expected: FAIL. The grandchild outlives the forget, and the hung adapter runs past the deadline.

- [ ] **Step 3: The narrowed texts, the pin, the checklist, one deadline**

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      let deadline = until.min(Instant::now() + ADAPTER_SHARE);
  ```

  with:

  ```rust
      let deadline = adapter_deadline(until, Instant::now());
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      adapter.terminate(ADAPTER_GRACE).await;
  }
  ```

  with:

  ```rust
      adapter.terminate(ADAPTER_GRACE).await;
  }

  /// When a forget's adapter is cut (B6; the review's item 4): its share of
  /// the deadline, and never later than the grace before `until`, so its
  /// SIGTERM-to-SIGKILL grace still ends by `until`, inside the collector's
  /// wait.
  pub(crate) fn adapter_deadline(until: Instant, now: Instant) -> Instant {
      let latest = until.checked_sub(ADAPTER_GRACE).unwrap_or(now).max(now);
      latest.min(now + ADAPTER_SHARE)
  }
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
          self.left.entry((kind, reason)).or_default();
      }
  ```

  with:

  ```rust
          self.left.entry((kind, reason)).or_default();
      }

      /// Whether anything was left for `reason`.
      pub(crate) fn has_left(&self, reason: ForgetReason) -> bool {
          self.left.keys().any(|(_, r)| *r == reason)
      }
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust

      /// The review's item 3: what the deadline left, a retry may finish.
  ```

  with:

  ```rust

      /// The review's item 4 (9d-ii): the adapter is cut its grace before
      /// the forget's deadline, or at its own share, whichever comes first.
      #[test]
      fn a_forgets_adapter_ends_its_grace_before_the_deadline() {
          let now = Instant::now();
          let until = now + Duration::from_secs(20);
          assert_eq!(adapter_deadline(until, now), now + ADAPTER_SHARE);
          let soon = now + Duration::from_secs(5);
          assert_eq!(adapter_deadline(soon, now), soon - ADAPTER_GRACE);
          assert_eq!(adapter_deadline(now + Duration::from_secs(1), now), now);
      }

      /// The review's item 3: what the deadline left, a retry may finish.
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust

  /// `thread/delete`'s answer, as Codex 0.155.1 gives it (its
  ```

  with:

  ```rust

  /// How 0.155.1 answers a request it could not deserialize (seen live):
  /// an unknown method, a missing field, a field of another type.
  const NOT_UNDERSTOOD: [&str; 3] = [
      "Invalid request: unknown variant",
      "Invalid request: missing field",
      "Invalid request: invalid type",
  ];

  /// `thread/delete`'s answer, as Codex 0.155.1 gives it (its
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
          // another shape. No handler ran.
          -32600 if message.starts_with("Invalid request: ") => Verdict::Unavailable(Unavailable::MethodNotFound),
  ```

  with:

  ```rust
          // another shape (the prefixes 0.155.1 was seen to give). No handler
          // ran.
          -32600 if NOT_UNDERSTOOD.iter().any(|p| message.starts_with(p)) => {
              Verdict::Unavailable(Unavailable::MethodNotFound)
          }
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
          tokio::task::spawn_blocking(move || present(&kinds, &id, &hooks))
              .await
              .unwrap_or(0)
      } else {
          0
  ```

  with:

  ```rust
          let until = until.into_std();
          tokio::task::spawn_blocking(move || present(&kinds, &id, &hooks, until))
              .await
              .unwrap_or(None)
      } else {
          Some(0)
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
              let walked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent);
  ```

  with:

  ```rust
              let until = until.into_std();
              let walked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent, until);
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
              let checked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, false, left_as);
                  let after = present(&kinds, &id, &hooks);
                  if before > after {
  ```

  with:

  ```rust
              let until = until.into_std();
              let checked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, false, left_as, until);
                  // Counted only from two whole counts: a walk the deadline
                  // cut proves nothing gone.
                  if let (Some(before), Some(after)) = (before, present(&kinds, &id, &hooks, until))
                      && before > after
                  {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
  fn rollouts(kinds: &Kinds, id: &str, hooks: &walk::Hooks, remove: bool, left_as: ForgetReason) -> Tally {
  ```

  with:

  ```rust
  fn rollouts(
      kinds: &Kinds,
      id: &str,
      hooks: &walk::Hooks,
      remove: bool,
      left_as: ForgetReason,
      until: std::time::Instant,
  ) -> Tally {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
                  max: depth,
              };
  ```

  with:

  ```rust
                  max: depth,
                  until,
              };
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      for (dir, path, depth) in kind_dirs(kinds) {
          let walker = Walk {
              id,
              dev: kinds.dev,
              hooks,
              max: depth,
          };
          walker.dir(
              dir,
              &path,
              0,
              &mut |dir, name, st, tally| {
  ```

  with:

  ```rust
      for (dir, path, depth) in kind_dirs(kinds) {
          let walker = Walk {
              id,
              dev: kinds.dev,
              hooks,
              max: depth,
              until,
          };
          walker.dir(
              dir,
              &path,
              0,
              &mut |dir, name, st, tally| {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
  /// How many of the session's rollout files (regular files) are there.
  fn present(kinds: &Kinds, id: &str, hooks: &walk::Hooks) -> u32 {
  ```

  with:

  ```rust
  /// How many of the session's rollout files (regular files) are there:
  /// `None` if the deadline cut the count short.
  fn present(kinds: &Kinds, id: &str, hooks: &walk::Hooks, until: std::time::Instant) -> Option<u32> {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
              max: depth,
          };
  ```

  with:

  ```rust
              max: depth,
              until,
          };
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      count
  ```

  with:

  ```rust
      (!scratch.has_left(ForgetReason::TimedOut)).then_some(count)
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      max: usize,
  }
  ```

  with:

  ```rust
      max: usize,
      /// The forget's one deadline (the review's item 4): past it the walk
      /// stops, and what it did not reach is left `timed_out`.
      until: std::time::Instant,
  }
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      fn dir(&self, dir: RawFd, path: &Path, depth: usize, on: &mut OnRollout<'_>, tally: &mut Tally) {
          let names = match self.hooks.list(dir, path) {
  ```

  with:

  ```rust
      fn dir(&self, dir: RawFd, path: &Path, depth: usize, on: &mut OnRollout<'_>, tally: &mut Tally) {
          if std::time::Instant::now() >= self.until {
              return tally.left(ForgetKind::Transcript, crate::forget::stop_reason(walk::Stop::Deadline));
          }
          let names = match self.hooks.list(dir, path) {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
          );
          for other in [
  ```

  with:

  ```rust
          );
          assert_eq!(
              classify_delete(&error(
                  -32600,
                  "Invalid request: invalid type: integer `1`, expected a string"
              )),
              Verdict::Unavailable(Unavailable::MethodNotFound)
          );
          // The review's item 2: only the prefixes seen live mean "not
          // understood"; another `Invalid request:` is a failure, retried.
          assert_eq!(
              classify_delete(&error(-32600, "Invalid request: something new")),
              Verdict::Failed(ForgetReason::IoError)
          );
          for other in [
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
                  max: SESSIONS_DEPTH,
              };
  ```

  with:

  ```rust
                  max: SESSIONS_DEPTH,
                  until: std::time::Instant::now() + Duration::from_secs(60),
              };
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      }

      #[test]
      fn date_directories_are_codexs_own_shape() {
  ```

  with:

  ```rust
      }

      /// The review's item 4: the walk keeps the forget's one deadline. Past
      /// it, nothing is listed or reported found; what is left is
      /// `timed_out`, retried.
      #[test]
      fn a_walk_past_its_deadline_stops_and_says_so() {
          let dir = tempfile::tempdir().unwrap();
          let day = dir.path().join("sessions/2026/10/02");
          std::fs::create_dir_all(&day).unwrap();
          std::fs::write(day.join(format!("rollout-2026-10-02T10-00-00-{ID}.jsonl")), "x").unwrap();
          let sessions = walk::open_root(&dir.path().join("sessions")).unwrap();
          let dev = walk::stat_fd(sessions.as_raw_fd()).unwrap().st_dev;
          let hooks = walk::Hooks::default();
          let walker = Walk {
              id: ID,
              dev,
              hooks: &hooks,
              max: SESSIONS_DEPTH,
              until: std::time::Instant::now(),
          };
          let (mut tally, mut found) = (Tally::default(), 0);
          walker.dir(
              sessions.as_raw_fd(),
              Path::new("sessions"),
              0,
              &mut |_, _, _, _| found += 1,
              &mut tally,
          );
          assert_eq!(found, 0);
          assert_eq!(
              tally.into_forgotten().remaining,
              [left(ForgetKind::Transcript, 1, ForgetReason::TimedOut, true)]
          );
      }

      /// The review's item 4: a count the deadline cut is no count, so no
      /// removal is claimed from it.
      #[test]
      fn a_count_past_its_deadline_is_none() {
          let dir = tempfile::tempdir().unwrap();
          let root = dir.path().canonicalize().unwrap();
          let day = root.join("sessions/2026/10/02");
          std::fs::create_dir_all(&day).unwrap();
          std::fs::write(day.join(format!("rollout-2026-10-02T10-00-00-{ID}.jsonl")), "x").unwrap();
          let ctx = ForgetContext {
              agents: std::collections::HashMap::new(),
              data_dir: PathBuf::from("/nonexistent-data"),
              home: None,
              hooks: walk::Hooks::default(),
              account: None,
              codex_app_server: None,
              codex_pin: None,
              deadline: crate::forget::FORGET_DEADLINE,
          };
          let kinds = check(&ctx, &root, &CODEX_KINDS).unwrap();
          let hooks = walk::Hooks::default();
          let later = std::time::Instant::now() + Duration::from_secs(60);
          assert_eq!(present(&kinds, ID, &hooks, later), Some(1));
          assert_eq!(present(&kinds, ID, &hooks, std::time::Instant::now()), None);
      }

      #[test]
      fn date_directories_are_codexs_own_shape() {
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust

  /// The app-server's call shape is read from one Codex version (plan 9d
  ```

  with:

  ```rust

  /// The npm package of Codex's CLI.
  const CODEX_PACKAGE: &str = "@openai/codex";

  /// The app-server's call shape is read from one Codex version (plan 9d
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      let name = package_name(path, entry)?;
      let version = entry
  ```

  with:

  ```rust
      let name = package_name(path, entry)?;
      // Codex's own CLI, never another package that happens to hold the
      // launcher's path (the review's item 5).
      if name != CODEX_PACKAGE {
          bail!(
              "codex_app_server: bin {:?} is in {name}, which is not {CODEX_PACKAGE}",
              pin.bin
          );
      }
      let version = entry
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      /// `PINS`, with a codex adapter bundling `@vendor/codex` and the
  ```

  with:

  ```rust
      /// `PINS`, with a codex adapter bundling `@openai/codex` and the
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
  cli = ["node_modules/@vendor/codex-"]
  [codex_app_server]
  codex_version = "0.155.1"
  bin = "node_modules/@vendor/codex/bin/codex.js"
  ```

  with:

  ```rust
  cli = ["node_modules/@openai/codex-"]
  [codex_app_server]
  codex_version = "0.155.1"
  bin = "node_modules/@openai/codex/bin/codex.js"
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
      /// `@vendor/codex` at `codex_version`.
  ```

  with:

  ```rust
      /// `@openai/codex` at `codex_version`.
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
              "node_modules/@vendor/codex",
              "@vendor/codex",
  ```

  with:

  ```rust
              "node_modules/@openai/codex",
              "@openai/codex",
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
                  &format!("node_modules/@vendor/codex-{suffix}"),
                  &format!("@vendor/codex-{suffix}"),
  ```

  with:

  ```rust
                  &format!("node_modules/@openai/codex-{suffix}"),
                  &format!("@openai/codex-{suffix}"),
  ```

  In `crates/hennery-pins/src/lib.rs`, replace:

  ```rust
          assert_eq!(pin.bin, "node_modules/@vendor/codex/bin/codex.js");
          let (pins, locks, registry) = codex_fixture("0.156.0");
          let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
          assert!(err.contains("bundles @vendor/codex 0.156.0"), "{err}");
  ```

  with:

  ```rust
          assert_eq!(pin.bin, "node_modules/@openai/codex/bin/codex.js");
          let (pins, locks, registry) = codex_fixture("0.156.0");
          let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
          assert!(err.contains("bundles @openai/codex 0.156.0"), "{err}");
          // A launcher in another package, even at the pinned version (the
          // review's item 5).
          let (mut pins, locks, registry) = codex_fixture("0.155.1");
          let pin = pins.codex_app_server.as_mut().unwrap();
          pin.bin = "node_modules/@acp/codex/dist/index.js".into();
          pin.codex_version = "1.0.0".into();
          let err = format!("{:#}", build(&pins, &locks, &registry, &node_archives()).unwrap_err());
          assert!(err.contains("is not @openai/codex"), "{err}");
  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-codex.rs`, replace:

  ```rust
          std::fs::write(path, std::process::id().to_string()).expect("write the pid file");
      }
  ```

  with:

  ```rust
          std::fs::write(path, std::process::id().to_string()).expect("write the pid file");
      }
      if let Some(path) = &script.grandchild_pid_file {
          // Never waited on: it must outlive the app-server unless the host
          // kills the whole group. It ignores SIGTERM (kept across `exec`),
          // so only the group's SIGKILL ends it.
          #[allow(clippy::zombie_processes)]
          let child = std::process::Command::new("/bin/sh")
              .args(["-c", "trap '' TERM; exec sleep 600"])
              .stdin(std::process::Stdio::null())
              .stdout(std::process::Stdio::null())
              .stderr(std::process::Stdio::null())
              .spawn()
              .expect("spawn the grandchild");
          std::fs::write(path, child.id().to_string()).expect("write the grandchild's pid");
      }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
      pub hang_version: bool,
  }
  ```

  with:

  ```rust
      pub hang_version: bool,
      /// The app-server starts a child of its own (`sleep`, in its process
      /// group, its stdio detached, SIGTERM ignored) and writes the child's
      /// pid here: for the group kill (the review's item 6).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub grandchild_pid_file: Option<String>,
  }
  ```

  In `packaging/README.md`, replace:

  ```markdown
    first and update `[codex_app_server]` in `pins.toml`.

  ```

  with:

  ```markdown
    first and update `[codex_app_server]` in `pins.toml`.
  - **What the host reads from Codex is still what it says** (the 9d-ii
    review). Re-read, in the bumped version's source and against a live run
    in a scratch `CODEX_HOME`:
    - the refusal texts `classify_delete` matches (`forked history still
      references it`, `thread is not persisted`, `live internal threads`,
      `no rollout found for thread id`, `thread not found:`, and the
      `Invalid request: unknown variant` / `missing field` / `invalid type`
      prefixes);
    - the `codexHome` field of `initialize`'s answer, canonical;
    - `--version`'s `codex-cli X.Y.Z` line.

  ```


- [ ] **Step 4: Run them to see them pass**

- [ ] **Step 5: Revert-probes**

Each of these is caught:
- each of the three `-32600` prefixes;
- the `@openai/codex` assertion;
- the walk's `until`;
- the adapter's capped share;
- the grandchild's kill.

The hung-adapter test caught the deadline probe, which the app-server test alone had not.

- [ ] **Step 6: The full checks**

- [ ] **Step 7: Commit**

### Task 3: Reopen the directories after the archive

- [ ] **Step 1: Write the failing test**

  In `crates/hennery-testkit/tests/forget_codex.rs`, replace:

  ```rust

  /// The review's item 4: a fallback whose adapter never answers is cut its
  ```

  with:

  ```rust

  /// The review's item 1: before a home's first archive there is no
  /// `archived_sessions/`; codex-acp's archive creates it and moves the
  /// rollouts there. The walk after the archive reopens the kind
  /// directories (with every B3 check) and removes them: nothing is left.
  #[tokio::test]
  async fn a_fallback_into_a_new_archived_sessions_leaves_nothing() {
      let root = Root::new();
      for rel in OWN.iter().filter(|r| r.starts_with("sessions/")) {
          let path = root.at(rel);
          std::fs::create_dir_all(path.parent().unwrap()).unwrap();
          std::fs::write(path, "rollout").unwrap();
      }
      assert!(!root.at("archived_sessions").exists());
      let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
      ctx.codex_app_server = None;
      let forgotten = root.forget_with(&ctx).await;
      assert!(root.archived().is_some(), "the archive ran");
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)],
          "{forgotten:?}"
      );
      assert_eq!(removed(&forgotten, ForgetKind::Transcript), 2);
      let left: Vec<_> = std::fs::read_dir(root.at("archived_sessions")).unwrap().collect();
      assert!(left.is_empty(), "{left:?}");
  }

  /// The review's item 1: the directories reopened after the archive pass
  /// every B3 check again. A root replaced while the adapter ran (another
  /// directory at the path, its device and inode not the first open's) is
  /// refused, `unsafe_root`, and nothing is removed from either; an
  /// `archived_sessions/` the archive left as a symlink is reported, never
  /// followed.
  #[tokio::test]
  async fn the_directories_reopened_after_the_archive_are_checked_again() {
      fn with_script(root: &Root, edit: impl FnOnce(&mut FakeScript)) -> ForgetContext {
          let mut ctx = root.ctx(root.fake(FakeDelete::Delete));
          ctx.codex_app_server = None;
          let mut script = FakeScript {
              codex_archive_log: Some(root.archive_log().to_str().unwrap().into()),
              ..FakeScript::default()
          };
          edit(&mut script);
          let adapter = ctx.agents.get_mut("codex").unwrap();
          adapter.env.retain(|(k, _)| k != SCRIPT_ENV);
          adapter
              .env
              .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
          ctx
      }
      // A root replaced.
      let root = Root::new();
      root.populate();
      let ctx = with_script(&root, |s| s.codex_archive_replaces_home = true);
      let forgotten = root.forget_with(&ctx).await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Session, ForgetReason::UnsafeRoot, false)],
          "{forgotten:?}"
      );
      let aside = root.root().with_extension("aside");
      let rollouts_in = |dir: &Path| {
          std::fs::read_dir(dir)
              .unwrap()
              .filter(|e| {
                  e.as_ref()
                      .unwrap()
                      .file_name()
                      .to_string_lossy()
                      .starts_with("rollout-")
              })
              .count()
      };
      assert_eq!(
          rollouts_in(&aside.join("archived_sessions")),
          OWN.len(),
          "nothing was removed from the moved-aside root"
      );
      // `archived_sessions/` left as a symlink.
      let root = Root::new();
      root.populate();
      let target = root.outside().join("archived");
      let ctx = with_script(&root, |s| {
          s.codex_archive_links_archived = Some(target.to_str().unwrap().into())
      });
      let forgotten = root.forget_with(&ctx).await;
      assert_eq!(
          reasons(&forgotten),
          [
              (ForgetKind::Transcript, ForgetReason::Symlink, false),
              (ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)
          ],
          "{forgotten:?}"
      );
      assert_eq!(rollouts_in(&target), OWN.len(), "the link was followed");
  }

  /// The review's item 4: a fallback whose adapter never answers is cut its
  ```


- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p hennery-testkit --locked --test forget_codex new_archived`
Expected: FAIL. The rollout is left in the new `archived_sessions/`, and the record closes.

- [ ] **Step 3: The second check and the reopen**

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust

      pub(crate) fn into_forgotten(self) -> Forgotten {
  ```

  with:

  ```rust

      /// Add what `other` counted.
      pub(crate) fn merge(&mut self, other: Tally) {
          for (kind, count) in other.removed {
              *self.removed.entry(kind).or_default() += count;
          }
          for (key, count) in other.left {
              *self.left.entry(key).or_default() += count;
          }
      }

      pub(crate) fn into_forgotten(self) -> Forgotten {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
              let hooks = ctx.hooks.clone();
              let until = until.into_std();
              let walked = tokio::task::spawn_blocking(move || {
                  let mut tally = rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent, until);
  ```

  with:

  ```rust
              // The archive may have created `archived_sessions/` (a home's
              // first) and moved the rollouts there: the kind directories are
              // opened again, with every B3 check, under the same root (the
              // review's item 1).
              let first = Arc::clone(&kinds);
              let rechecked = {
                  let (ctx, root) = (ctx.clone(), root.clone());
                  tokio::task::spawn_blocking(move || check(&ctx, &root, &CODEX_KINDS)).await
              };
              let kinds = match rechecked {
                  Ok(Ok(kinds)) if same_dir(&first.root, &kinds.root) => kinds,
                  Ok(Ok(_)) => return whole(ForgetReason::UnsafeRoot),
                  Ok(Err(reason)) => return whole(reason),
                  Err(_) => return whole(ForgetReason::IoError),
              };
              drop(first);
              let hooks = ctx.hooks.clone();
              let until = until.into_std();
              let walked = tokio::task::spawn_blocking(move || {
                  let mut tally = Tally::default();
                  for (kind, _, opened) in &kinds.dirs {
                      if let Err(reason) = opened {
                          tally.left(*kind, *reason);
                      }
                  }
                  tally.merge(rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent, until));

  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
                  Err(_) => whole(ForgetReason::IoError),
              }
          }
      }
  }

  ```

  with:

  ```rust
                  Err(_) => whole(ForgetReason::IoError),
              }
          }
      }
  }

  /// Whether two open descriptors are the same directory (`st_dev`,
  /// `st_ino`).
  fn same_dir(a: &std::os::fd::OwnedFd, b: &std::os::fd::OwnedFd) -> bool {
      match (walk::stat_fd(a.as_raw_fd()), walk::stat_fd(b.as_raw_fd())) {
          (Ok(a), Ok(b)) => (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino),
          _ => false,
      }
  }

  ```

  In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
          std::fs::rename(&path, archived.join(name)).map_err(|e| e.to_string())?;
      }
      Ok(())
  }
  ```

  with:

  ```rust
          std::fs::rename(&path, archived.join(name)).map_err(|e| e.to_string())?;
      }
      if let Some(target) = &script.codex_archive_links_archived {
          std::fs::rename(&archived, target).map_err(|e| e.to_string())?;
          std::os::unix::fs::symlink(target, &archived).map_err(|e| e.to_string())?;
      }
      if script.codex_archive_replaces_home {
          let aside = home.with_extension("aside");
          std::fs::rename(home, &aside).map_err(|e| e.to_string())?;
          std::fs::create_dir(home).map_err(|e| e.to_string())?;
      }
      Ok(())
  }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
      pub codex_archive_log: Option<String>,
  }
  ```

  with:

  ```rust
      pub codex_archive_log: Option<String>,
      /// After the archive, move `$CODEX_HOME` aside and make a fresh,
      /// empty directory in its place: a root swapped while the adapter ran.
      #[serde(default, skip_serializing_if = "std::ops::Not::not")]
      pub codex_archive_replaces_home: bool,
      /// After the archive, move `archived_sessions/` here and leave a
      /// symlink to it in its place.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub codex_archive_links_archived: Option<String>,
  }
  ```

  In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
              codex_archive_log: None,
          }
  ```

  with:

  ```rust
              codex_archive_log: None,
              codex_archive_replaces_home: false,
              codex_archive_links_archived: None,
          }
  ```


- [ ] **Step 4: Run it to see it pass**

- [ ] **Step 5: Revert-probes**

Each of these is caught:
- the second `check`;
- the root's device-and-inode comparison;
- walking the reopened directories rather than the first ones;
- a flagged fallback skipping B3's refusal. The fresh implementer found this one in the WIP.

- [ ] **Step 6: The full checks**

- [ ] **Step 7: Commit**

### Task 4: The fallback after three timeouts

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-proto/tests/frames.rs`, replace:

  ```rust
              sqlite_root: None,
          },
      };
      let expected = json!({
  ```

  with:

  ```rust
              sqlite_root: None,
          },
          fallback: false,
      };
      let expected = json!({
  ```

  In `crates/hennery-proto/tests/frames.rs`, replace:

  ```rust
          Ok(Some(hennery_proto::frames::Capability::ForgetSession))
      );
  ```

  with:

  ```rust
          Ok(Some(hennery_proto::frames::Capability::ForgetSession))
      );
      // Plan 9d-ii's hybrid: `fallback` only when set, absent otherwise.
      let flagged = CollectorFrame::ForgetSession {
          request_id: "r".into(),
          agent: "codex".into(),
          agent_session_id: "a1".into(),
          agent_home: AgentHome {
              root: "/h/.codex".into(),
              sqlite_root: None,
          },
          fallback: true,
      };
      let flagged_json = json!({
          "type": "forget_session", "request_id": "r", "agent": "codex", "agent_session_id": "a1",
          "agent_home": {"root": "/h/.codex"}, "fallback": true
      });
      assert_eq!(serde_json::to_value(&flagged).unwrap(), flagged_json);
      assert_eq!(serde_json::from_value::<CollectorFrame>(flagged_json).unwrap(), flagged);
      assert_eq!(
          serde_json::to_value(ForgetReason::AppServerTimedOut).unwrap(),
          json!("app_server_timed_out")
      );
  ```

  In `crates/hennery-testkit/tests/forget.rs`, replace:

  ```rust

  /// Decision 5: a delete while the host is away answers `pending,
  ```

  with:

  ```rust

  /// B5 as ruled (the hybrid) end to end: a record whose app-server timed
  /// out `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` times in a row is sent
  /// flagged `fallback` at its host's return, and the host spawns no Codex:
  /// the archive and the walk only, the session's own rollout gone, the
  /// other thread's kept, and the record final with `codex_database_copies`.
  #[tokio::test]
  async fn a_record_past_the_app_server_timeouts_is_forgotten_by_the_fallback_alone() {
      let roots = Roots::new();
      let db = roots.base.join("hennery.db");
      let collector = Collector::start(&db).await;
      let log = roots.base.join("codex.log");
      let archive_log = roots.base.join("archive.log");
      let script = FakeScript {
          codex_archive_log: Some(archive_log.to_str().unwrap().into()),
          ..answering(AGENT_SESSION)
      };
      let mut cfg = host_config(collector.addr, &roots, &script);
      cfg.codex_app_server = Some(fake_codex(&log));
      let host = start_host(cfg.clone());
      connected(&collector, true).await;
      let session = start_session(&collector, "codex", &roots).await;
      let (own, other) = codex_rollouts(&roots.codex());
      host.abort();
      connected(&collector, false).await;
      let (result, body) = deleted(&collector, &session).await;
      assert_eq!(result.host_transcript.state, RemovalState::Pending, "{body}");
      rusqlite::Connection::open(&db)
          .unwrap()
          .execute(
              "UPDATE host_forgets SET app_server_timeouts = ?1",
              [hennery_sessions::forget::APP_SERVER_TIMEOUTS_BEFORE_FALLBACK],
          )
          .unwrap();
      start_host(cfg);
      let item = wait_for("the flagged retry at the host's return", || async {
          let listed = removals(&collector).await;
          (listed[0].state == HostRemovalState::Final).then(|| listed[0].clone())
      })
      .await;
      let result = item.last_result.expect("a result");
      assert_eq!(
          result.remaining.iter().map(|r| (r.kind, r.reason)).collect::<Vec<_>>(),
          [(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly)],
          "{result:?}"
      );
      assert!(!own.exists() && other.exists());
      assert!(!log.exists(), "a Codex was spawned");
      assert!(std::fs::read_to_string(&archive_log).unwrap().contains("CODEX_HOME="));
  }

  /// Decision 5: a delete while the host is away answers `pending,
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
              },
          }
  ```

  with:

  ```rust
              },
              fallback: false,
          }
  ```

  In `crates/hennery-testkit/tests/forget_claude.rs`, replace:

  ```rust
          assert!(root.at(&format!("tasks/{ID}")).exists());
      }
  }
  ```

  with:

  ```rust
          assert!(root.at(&format!("tasks/{ID}")).exists());
      }
  }

  /// Plan 9d-ii's hybrid: `fallback` is Codex's alone. A Claude forget that
  /// carries it (forged, say) is the same forget: the adapter's delete and
  /// the exact entries, nothing more and nothing less.
  #[tokio::test]
  async fn a_claude_forget_flagged_fallback_is_unchanged() {
      let root = Root::new();
      root.populate();
      let mut flagged = root.forget_at(&root.root());
      flagged.fallback = true;
      let forgotten = forget(&root.ctx(Hooks::default()), &flagged).await;
      assert_eq!(reasons(&forgotten), [], "{forgotten:?}");
      assert!(root.adapter_ran().is_some(), "the adapter's delete ran");
      let unflagged = Root::new();
      unflagged.populate();
      let same = unflagged.forget().await;
      assert_eq!((forgotten, root.tree()), (same, unflagged.tree()));
  }
  ```

  In `crates/hennery-testkit/tests/forget_codex.rs`, replace:

  ```rust
              },
          }
  ```

  with:

  ```rust
              },
              fallback: false,
          }
  ```

  In `crates/hennery-testkit/tests/forget_codex.rs`, replace:

  ```rust
          assert_eq!(
              reasons(&forgotten),
              [
                  (ForgetKind::Session, ForgetReason::TimedOut, true),
                  (ForgetKind::Transcript, ForgetReason::TimedOut, true)
  ```

  with:

  ```rust
          // Before the delete was written, the app-server's own timeout (the
          // hybrid's count, B5 as ruled); after, the delete's.
          let reason = if frames == 2 {
              ForgetReason::TimedOut
          } else {
              ForgetReason::AppServerTimedOut
          };
          assert_eq!(
              reasons(&forgotten),
              [
                  (ForgetKind::Session, reason, true),
                  (ForgetKind::Transcript, reason, true)
  ```

  In `crates/hennery-testkit/tests/forget_codex.rs`, replace:

  ```rust

  /// B3, decision 12: `sessions/` or `archived_sessions/` that is a symlink
  ```

  with:

  ```rust

  /// B5 as ruled (the hybrid): a forget the collector flags `fallback`
  /// spawns no Codex at all, and goes straight to the archive and the walk
  /// under the recorded home, after every B3 check: the same result as an
  /// unavailable app-server, final. A flag only downgrades this session's
  /// own deletion: the other thread's rollouts and everything else stay,
  /// and a failed check still spawns nothing.
  #[tokio::test]
  async fn a_forget_flagged_fallback_spawns_no_codex_and_removes_only_its_own() {
      let root = Root::new();
      root.populate();
      let mut flagged = root.forget_at(&root.root(), None);
      flagged.fallback = true;
      let forgotten = forget(&root.ctx(root.fake(FakeDelete::Delete)), &flagged).await;
      assert_fell_back(&root, &forgotten, "flagged");
      assert_eq!(root.spawns().len(), 0, "{:?}", root.spawns());
      // A flagged forget whose kind directory fails its check: nothing runs.
      let root = Root::new();
      let target = root.outside().join("real");
      std::fs::create_dir_all(&target).unwrap();
      symlink(&target, root.at("sessions")).unwrap();
      let mut flagged = root.forget_at(&root.root(), None);
      flagged.fallback = true;
      let forgotten = forget(&root.ctx(root.fake(FakeDelete::Delete)), &flagged).await;
      assert_eq!(
          reasons(&forgotten),
          [(ForgetKind::Transcript, ForgetReason::Symlink, false)],
          "{forgotten:?}"
      );
      assert_eq!((root.spawns().len(), root.archived()), (0, None));
  }

  /// B3, decision 12: `sessions/` or `archived_sessions/` that is a symlink
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
      async fn forget(&mut self, request_id: &str, agent_session_id: &str, root: &Path) -> Result<HostFrame, String> {
          self.send(&CollectorFrame::ForgetSession {
  ```

  with:

  ```rust
      async fn forget(&mut self, request_id: &str, agent_session_id: &str, root: &Path) -> Result<HostFrame, String> {
          self.forget_flagged(request_id, agent_session_id, root, false).await
      }

      /// `forget`, with the hybrid's `fallback` flag as given (plan 9d-ii).
      async fn forget_flagged(
          &mut self,
          request_id: &str,
          agent_session_id: &str,
          root: &Path,
          fallback: bool,
      ) -> Result<HostFrame, String> {
          self.send(&CollectorFrame::ForgetSession {
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
              agent_home: home(root),
          })
  ```

  with:

  ```rust
              agent_home: home(root),
              fallback,
          })
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
      );
      let ran = collector.forget("f5", AGENT_SESSION, &setup.root()).await.unwrap();
  ```

  with:

  ```rust
      );
      // Plan 9d-ii's hybrid: a forged `fallback` buys nothing past the same
      // checks. Another root, another id, an id the agent never writes: each
      // refused as unflagged.
      for (id, root) in [(AGENT_SESSION, setup.base.join("elsewhere")), (other_id, setup.root())] {
          let flagged = collector.forget_flagged("f4b", id, &root, true).await.unwrap();
          assert_eq!(
              reasons(&flagged),
              [(ForgetKind::Session, ForgetReason::UnknownToHost, false)]
          );
      }
      assert_eq!(
          collector.forget_flagged("f4c", "../../etc", &setup.root(), true).await,
          Err("invalid".into())
      );
      let ran = collector.forget("f5", AGENT_SESSION, &setup.root()).await.unwrap();
  ```

  In `crates/hennery-testkit/tests/forget_host.rs`, replace:

  ```rust
              agent_home: home(&setup.root()),
          })
  ```

  with:

  ```rust
              agent_home: home(&setup.root()),
              fallback: false,
          })
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
  async fn expect_forget(host: &mut ScriptedHost) -> String {
      let CollectorFrame::ForgetSession {
  ```

  with:

  ```rust
  async fn expect_forget(host: &mut ScriptedHost) -> String {
      let (request_id, fallback) = expect_forget_flag(host).await;
      assert!(!fallback, "a forget flagged fallback");
      request_id
  }

  /// `expect_forget`, with the hybrid's `fallback` flag it carried (plan
  /// 9d-ii).
  async fn expect_forget_flag(host: &mut ScriptedHost) -> (String, bool) {
      let CollectorFrame::ForgetSession {
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          agent_home: home,
      } = host.next().await
  ```

  with:

  ```rust
          agent_home: home,
          fallback,
      } = host.next().await
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      );
      request_id
  }
  ```

  with:

  ```rust
      );
      (request_id, fallback)
  }
  ```

  In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      assert!(listed.contains(&("s-old".to_string(), hennery_proto::rest::HostRemovalState::Final)));
  }
  ```

  with:

  ```rust
      assert!(listed.contains(&("s-old".to_string(), hennery_proto::rest::HostRemovalState::Final)));
  }

  /// Plan 9d-ii, B5 as ruled (the hybrid): consecutive answers whose whole
  /// forget timed out in the app-server (`app_server_timed_out`) are counted
  /// on the record; any other answer resets the count; once it reaches
  /// `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK`, the next `forget_session` carries
  /// `fallback`, and its final answer closes the record.
  #[tokio::test]
  async fn app_server_timeouts_in_a_row_flag_the_next_forget_fallback() {
      use hennery_proto::frames::{ForgetKind, ForgetReason, ForgetRemaining, ForgetWhat};
      use hennery_sessions::forget::APP_SERVER_TIMEOUTS_BEFORE_FALLBACK;
      let left = |kind, reason, retry| ForgetRemaining {
          what: ForgetWhat { kind, count: 0 },
          reason,
          retry,
      };
      let timed_out = || left(ForgetKind::Session, ForgetReason::AppServerTimedOut, true);
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      // The delete's own attempt: timed out in the app-server, once.
      let (first, fallback) = expect_forget_flag(&mut host).await;
      assert!(!fallback);
      host.send(&forgotten(first, vec![timed_out()])).await;
      let (status, body) = call.await.unwrap();
      assert_eq!(
          (status, body["host_transcript"]["state"].as_str()),
          (200, Some("partial")),
          "{body}"
      );
      assert_eq!(APP_SERVER_TIMEOUTS_BEFORE_FALLBACK, 3);
      let record = || collector.state.store.forgets_to_send(HOST).unwrap().remove(0);
      assert_eq!(record().app_server_timeouts, 1);
      // A timeout of the delete itself is no app-server timeout: it resets.
      let state = collector.state.clone();
      let r = record();
      let retry =
          tokio::spawn(async move { hennery_sessions::forget::attempt(&state, &r, Duration::from_secs(5)).await });
      let (id, fallback) = expect_forget_flag(&mut host).await;
      assert!(!fallback);
      host.send(&forgotten(
          id,
          vec![left(ForgetKind::Session, ForgetReason::TimedOut, true)],
      ))
      .await;
      retry.await.unwrap();
      assert_eq!(record().app_server_timeouts, 0, "reset");
      // Three in a row; only the fourth is flagged.
      for n in 1..=APP_SERVER_TIMEOUTS_BEFORE_FALLBACK {
          let state = collector.state.clone();
          let r = record();
          let retry =
              tokio::spawn(async move { hennery_sessions::forget::attempt(&state, &r, Duration::from_secs(5)).await });
          let (id, fallback) = expect_forget_flag(&mut host).await;
          assert!(!fallback, "attempt {n}");
          host.send(&forgotten(id, vec![timed_out()])).await;
          retry.await.unwrap();
          assert_eq!(record().app_server_timeouts, n);
      }
      let state = collector.state.clone();
      let r = record();
      let retry =
          tokio::spawn(async move { hennery_sessions::forget::attempt(&state, &r, Duration::from_secs(5)).await });
      let (id, fallback) = expect_forget_flag(&mut host).await;
      assert!(fallback, "the threshold flags the next forget");
      host.send(&forgotten(
          id,
          vec![left(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)],
      ))
      .await;
      let result = retry.await.unwrap();
      assert_eq!(result.state, hennery_proto::rest::RemovalState::Partial);
      let listed = removals(&collector).await;
      assert_eq!(listed[0].state, hennery_proto::rest::HostRemovalState::Final);
  }

  /// The hybrid's count across an attempt's second round (plan 9d-ii): an
  /// attempt asked again while in flight sends the count its own first
  /// answer made, not the one it started with. Here the first answer makes
  /// it the threshold, so the second round is flagged.
  #[tokio::test]
  async fn an_attempt_asked_again_sends_the_count_its_first_answer_made() {
      use hennery_proto::frames::{ForgetKind, ForgetReason, ForgetRemaining, ForgetWhat};
      use hennery_sessions::forget::{APP_SERVER_TIMEOUTS_BEFORE_FALLBACK, attempt};
      let left = |kind, reason, retry| ForgetRemaining {
          what: ForgetWhat { kind, count: 0 },
          reason,
          retry,
      };
      let timed_out = || left(ForgetKind::Session, ForgetReason::AppServerTimedOut, true);
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect_with(&collector, vec![], 0, forgetting()).await;
      let session = parked_claude_session(&collector, &mut host).await;
      let c = client(&collector);
      let url = session_url(&collector, &session);
      let call = tokio::spawn(async move { delete(&c, url).await });
      let (first, _) = expect_forget_flag(&mut host).await;
      host.send(&forgotten(first, vec![timed_out()])).await;
      call.await.unwrap();
      let record = || collector.state.store.forgets_to_send(HOST).unwrap().remove(0);
      // Up to one short of the threshold.
      while record().app_server_timeouts < APP_SERVER_TIMEOUTS_BEFORE_FALLBACK - 1 {
          let (state, r) = (collector.state.clone(), record());
          let retry = tokio::spawn(async move { attempt(&state, &r, Duration::from_secs(5)).await });
          let (id, fallback) = expect_forget_flag(&mut host).await;
          assert!(!fallback);
          host.send(&forgotten(id, vec![timed_out()])).await;
          retry.await.unwrap();
      }
      // An attempt in flight, and another asked for meanwhile.
      let (state, r) = (collector.state.clone(), record());
      let held = tokio::spawn(async move { attempt(&state, &r, Duration::from_secs(10)).await });
      let (id, fallback) = expect_forget_flag(&mut host).await;
      assert!(!fallback);
      let asked = attempt(&collector.state, &record(), Duration::from_secs(5)).await;
      assert_eq!(asked.pending, Some(hennery_proto::rest::RemovalPending::InProgress));
      host.send(&forgotten(id, vec![timed_out()])).await;
      // The holder's second round: the threshold reached, flagged.
      let (id, fallback) = expect_forget_flag(&mut host).await;
      assert!(fallback, "the second round sent a stale count");
      host.send(&forgotten(
          id,
          vec![left(ForgetKind::CodexDatabaseCopies, ForgetReason::FallbackOnly, false)],
      ))
      .await;
      assert_eq!(held.await.unwrap().state, hennery_proto::rest::RemovalState::Partial);
  }
  ```


- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit -p hennery-sessions --locked --test forget_codex --test forget`
Expected: FAIL to compile: `no variant AppServerTimedOut`, and no field `fallback`.

- [ ] **Step 3: The count, the flag, and the flagged path**

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
              agent_home,
          } => forget(
  ```

  with:

  ```rust
              agent_home,
              fallback,
          } => forget(
  ```

  In `crates/hennery-host/src/connection.rs`, replace:

  ```rust
                  agent_home,
              },
  ```

  with:

  ```rust
                  agent_home,
                  fallback,
              },
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
      pub agent_home: hennery_proto::frames::AgentHome,
  }
  ```

  with:

  ```rust
      pub agent_home: hennery_proto::frames::AgentHome,
      /// The collector's `fallback` (plan 9d-ii's hybrid): for Codex, no
      /// app-server, the fallback at once. Ignored for any other agent.
      pub fallback: bool,
  }
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
          ForgetReason::StillPresent | ForgetReason::IoError | ForgetReason::TimedOut | ForgetReason::InProgress
  ```

  with:

  ```rust
          ForgetReason::StillPresent
              | ForgetReason::IoError
              | ForgetReason::TimedOut
              | ForgetReason::AppServerTimedOut
              | ForgetReason::InProgress
  ```

  In `crates/hennery-host/src/forget.rs`, replace:

  ```rust
                  },
              },
  ```

  with:

  ```rust
                  },
                  fallback: false,
              },
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
  //!    this Codex does not know. Never after `thread/delete` itself
  //!    answered: a refusal, a failure, or no answer in time. Nor when a step
  //!    only ran out of time (a first exec macOS scans, say): that is
  //!    `timed_out`, retried, since the fallback's result is final.
  ```

  with:

  ```rust
  //!    this Codex does not know. Never after `thread/delete` was written: a
  //!    refusal, a failure, or no answer in time (`timed_out`, retried).
  //! 5. **A slow app-server** (B5 as ruled, the bounded hybrid): a step that
  //!    only ran out of time before `thread/delete` was written (`--version`,
  //!    the start, `initialize`; a first exec macOS scans, say) is
  //!    `app_server_timed_out`, retried, since the fallback's result is final.
  //!    The collector counts such answers in a row; after
  //!    `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` it flags the next forget
  //!    `fallback`, and the host then spawns no Codex and runs the fallback
  //!    at once, after the same checks.
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      /// gave no answer in time. Retried; no fallback (it may have run).
  ```

  with:

  ```rust
      /// gave no answer in time (`timed_out`). Retried; no fallback (it may
      /// have run). Or the app-server ran out of time before it was sent
      /// (`app_server_timed_out`): retried, and counted by the collector.
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      MethodNotFound,
  }
  ```

  with:

  ```rust
      MethodNotFound,
      /// The collector flagged the forget `fallback` (B5 as ruled, the
      /// hybrid): its app-server timed out too often in a row. No Codex is
      /// spawned.
      Flagged,
  }
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      let before = if ctx.codex_pin.is_some() && ctx.codex_app_server.is_some() {
  ```

  with:

  ```rust
      let before = if !forget.fallback && ctx.codex_pin.is_some() && ctx.codex_app_server.is_some() {
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
      let verdict = app_server(ctx, forget, &root, &env, &strip, share).await;
  ```

  with:

  ```rust
      let verdict = if forget.fallback {
          Verdict::Unavailable(Unavailable::Flagged)
      } else {
          app_server(ctx, forget, &root, &env, &strip, share).await
      };
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
                  tally.merge(rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent, until));

                  // Decision 10: what only `thread/delete` reaches.
  ```

  with:

  ```rust
                  tally.merge(rollouts(&kinds, &id, &hooks, true, ForgetReason::StillPresent, until));
                  // Decision 10: what only `thread/delete` reaches.
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
          Err(_) => Err(Verdict::Failed(ForgetReason::TimedOut)),
  ```

  with:

  ```rust
          Err(_) => Err(Verdict::Failed(ForgetReason::AppServerTimedOut)),
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
          Err(_) => return Verdict::Failed(ForgetReason::TimedOut),
  ```

  with:

  ```rust
          // Before `thread/delete` was written: the app-server's own
          // timeout, counted by the collector (B5 as ruled).
          Err(_) => return Verdict::Failed(ForgetReason::AppServerTimedOut),
  ```

  In `crates/hennery-host/src/forget_codex.rs`, replace:

  ```rust
              (ForgetReason::TimedOut, true),
              (ForgetReason::StillPresent, true),
  ```

  with:

  ```rust
              (ForgetReason::TimedOut, true),
              (ForgetReason::AppServerTimedOut, true),
              (ForgetReason::StillPresent, true),
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      /// The forget's deadline passed before the removal was done (B6).
      TimedOut,
  ```

  with:

  ```rust
      /// The forget's deadline passed before the removal was done (B6). For
      /// Codex: after `thread/delete` was written, so it may have run.
      TimedOut,
      /// Codex's app-server did not answer in time before `thread/delete`
      /// was written (`--version`, its start, `initialize`): retried, and
      /// counted by the collector, which flags the record's next forget
      /// `fallback` after `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` in a row
      /// (plan 9d-ii, B5 as ruled).
      AppServerTimedOut,
  ```

  In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
          agent_home: AgentHome,
      },
  ```

  with:

  ```rust
          agent_home: AgentHome,
          /// Plan 9d-ii, B5 as ruled (the hybrid): the record's last
          /// `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` answers were all
          /// `app_server_timed_out`, so a Codex host spawns no Codex and runs
          /// the fallback at once, after the same checks. Only ever a
          /// downgrade of this session's own removal; other agents ignore it.
          /// Absent when false.
          #[serde(default, skip_serializing_if = "std::ops::Not::not")]
          fallback: bool,
      },
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
  pub const FORGET_WAIT: Duration = Duration::from_secs(30);

  ```

  with:

  ```rust
  pub const FORGET_WAIT: Duration = Duration::from_secs(30);

  /// How many `app_server_timed_out` answers in a row flag a record's next
  /// forget `fallback` (plan 9d-ii, B5 as ruled).
  pub const APP_SERVER_TIMEOUTS_BEFORE_FALLBACK: u32 = 3;

  /// Whether `result` is one the hybrid counts (plan 9d-ii, B5 as ruled):
  /// its `session` item is `app_server_timed_out`. Any other result resets
  /// the count.
  pub fn app_server_timed_out(result: &TranscriptRemoval) -> bool {
      result
          .remaining
          .iter()
          .any(|r| r.kind == ForgetKind::Session && r.reason == ForgetReason::AppServerTimedOut)
  }

  /// A record's count after `result` (the hybrid's): one more, or 0.
  pub fn app_server_timeouts_after(count: u32, result: &TranscriptRemoval) -> u32 {
      if app_server_timed_out(result) {
          count.saturating_add(1)
      } else {
          0
      }
  }

  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
      loop {
          let (result, done) = attempt_once(
              state,
              record,
  ```

  with:

  ```rust
      // The claim makes this the record's only attempt in flight, so the
      // hybrid's count is followed here as the store keeps it: a second
      // round sends what the first one's answer made it (plan 9d-ii).
      let mut record = record.clone();
      loop {
          let (result, done) = attempt_once(
              state,
              &record,
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
              return result;
          }
      }
  }
  ```

  with:

  ```rust
              return result;
          }
          record.app_server_timeouts = app_server_timeouts_after(record.app_server_timeouts, &result);
      }
  }
  ```

  In `crates/hennery-sessions/src/forget.rs`, replace:

  ```rust
          agent_home: home,
      };
  ```

  with:

  ```rust
          agent_home: home,
          // B5 as ruled (the hybrid): after this many app-server timeouts in
          // a row, the host is told to go straight to the fallback.
          fallback: record.app_server_timeouts >= APP_SERVER_TIMEOUTS_BEFORE_FALLBACK,
      };
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  ",
  ];
  ```

  with:

  ```rust
  ",
      // Plan 9d-ii, B5 as ruled (the hybrid): how many of a record's answers
      // in a row were `app_server_timed_out`; any other answer resets it. At
      // `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` the next forget is flagged
      // `fallback`. No backfill: every record starts at 0.
      "
      ALTER TABLE host_forgets ADD COLUMN app_server_timeouts INTEGER NOT NULL DEFAULT 0;
  ",
  ];
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      pub created_at: String,
  }
  ```

  with:

  ```rust
      pub created_at: String,
      /// Answers in a row that were `app_server_timed_out` (plan 9d-ii).
      pub app_server_timeouts: u32,
  }
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  const FORGET_COLUMNS: &str =
      "id, host_id, session_id, agent, agent_session_id, agent_home, state, attempts, last_result, created_at";
  ```

  with:

  ```rust
  const FORGET_COLUMNS: &str = "id, host_id, session_id, agent, agent_session_id, agent_home, state, attempts, last_result, created_at, app_server_timeouts";
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              created_at: r.get(9)?,
          },
  ```

  with:

  ```rust
              created_at: r.get(9)?,
              app_server_timeouts: r.get(10)?,
          },
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              created_at: ts.clone(),
          };
  ```

  with:

  ```rust
              created_at: ts.clone(),
              app_server_timeouts: 0,
          };
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          // A final record keeps no roots (the review's item 12).
          conn.execute(
              "UPDATE host_forgets SET attempts = attempts + ?2, last_result = ?3, state = ?4,
                   agent_home = CASE WHEN ?4 = 'final' THEN NULL ELSE agent_home END
  ```

  with:

  ```rust
          // A final record keeps no roots (the review's item 12). The
          // hybrid's count (plan 9d-ii): one more for an answer whose whole
          // forget timed out in the app-server, back to 0 for any other.
          let app_server_timed_out = crate::forget::app_server_timed_out(result);
          conn.execute(
              "UPDATE host_forgets SET attempts = attempts + ?2, last_result = ?3, state = ?4,
                   agent_home = CASE WHEN ?4 = 'final' THEN NULL ELSE agent_home END,
                   app_server_timeouts = CASE WHEN ?6 THEN app_server_timeouts + 1 ELSE 0 END
  ```

  In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  if done { "final" } else { "pending" },
                  self.owner
              ],
  ```

  with:

  ```rust
                  if done { "final" } else { "pending" },
                  self.owner,
                  app_server_timed_out
              ],
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
              "agent_session_id": {
                "type": "string"
              },
              "request_id": {
                "type": "string"
              },
  ```

  with:

  ```json
              "agent_session_id": {
                "type": "string"
              },
              "fallback": {
                "description": "Plan 9d-ii, B5 as ruled (the hybrid): the record's last\n`APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` answers were all\n`app_server_timed_out`, so a Codex host spawns no Codex and runs\nthe fallback at once, after the same checks. Only ever a\ndowngrade of this session's own removal; other agents ignore it.\nAbsent when false.",
                "type": "boolean"
              },
              "request_id": {
                "type": "string"
              },
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
            "description": "The forget's deadline passed before the removal was done (B6).",
  ```

  with:

  ```json
            "description": "The forget's deadline passed before the removal was done (B6). For\nCodex: after `thread/delete` was written, so it may have run.",
            "type": "string"
          },
          {
            "const": "app_server_timed_out",
            "description": "Codex's app-server did not answer in time before `thread/delete`\nwas written (`--version`, its start, `initialize`): retried, and\ncounted by the collector, which flags the record's next forget\n`fallback` after `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` in a row\n(plan 9d-ii, B5 as ruled).",
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  export type ForgetReason = "attached" | "in_progress" | "unsupported_agent" | "no_recorded_home" | "unknown_to_host" | "shared" | "unsafe_root" | "root_missing" | "symlink" | "not_a_directory" | "unsafe_directory" | "mount_point" | "too_deep" | "timed_out" | "still_present" | "io_error" | "forked_history" | "ephemeral" | "home_mismatch" | "fallback_only" | "invalid_id" | "host_revoked";
  ```

  with:

  ```typescript
  export type ForgetReason = "attached" | "in_progress" | "unsupported_agent" | "no_recorded_home" | "unknown_to_host" | "shared" | "unsafe_root" | "root_missing" | "symlink" | "not_a_directory" | "unsafe_directory" | "mount_point" | "too_deep" | "timed_out" | "app_server_timed_out" | "still_present" | "io_error" | "forked_history" | "ephemeral" | "home_mismatch" | "fallback_only" | "invalid_id" | "host_revoked";
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```typescript
  content: unknown[], } | { "type": "cancel_turn", request_id: string, session_id: string, turn_id: string, } | { "type": "set_config", request_id: string, session_id: string, config_id: string, value: ConfigValue, } | { "type": "answer_permission", request_id: string, session_id: string, pending_id: string, option_id: string, } | { "type": "answer_elicitation", request_id: string, session_id: string, pending_id: string, action: ElicitationAction, content?: unknown, } | { "type": "ack", session_id: string, ack_seq: number, } | { "type": "resolve_path", request_id: string, path: string, } | { "type": "park_session", request_id: string, session_id: string, } | { "type": "close_session", request_id: string, session_id: string, } | { "type": "list_projects", request_id: string, } | { "type": "browse_directory", request_id: string, path: string, } | { "type": "forget_hat", hat_id: string, } | { "type": "forget_session", request_id: string, agent: string, agent_session_id: string, agent_home: AgentHome, };
  ```

  with:

  ```typescript
  content: unknown[], } | { "type": "cancel_turn", request_id: string, session_id: string, turn_id: string, } | { "type": "set_config", request_id: string, session_id: string, config_id: string, value: ConfigValue, } | { "type": "answer_permission", request_id: string, session_id: string, pending_id: string, option_id: string, } | { "type": "answer_elicitation", request_id: string, session_id: string, pending_id: string, action: ElicitationAction, content?: unknown, } | { "type": "ack", session_id: string, ack_seq: number, } | { "type": "resolve_path", request_id: string, path: string, } | { "type": "park_session", request_id: string, session_id: string, } | { "type": "close_session", request_id: string, session_id: string, } | { "type": "list_projects", request_id: string, } | { "type": "browse_directory", request_id: string, path: string, } | { "type": "forget_hat", hat_id: string, } | { "type": "forget_session", request_id: string, agent: string, agent_session_id: string, agent_home: AgentHome, 
  /**
   * Plan 9d-ii, B5 as ruled (the hybrid): the record's last
   * `APP_SERVER_TIMEOUTS_BEFORE_FALLBACK` answers were all
   * `app_server_timed_out`, so a Codex host spawns no Codex and runs
   * the fallback at once, after the same checks. Only ever a
   * downgrade of this session's own removal; other agents ignore it.
   * Absent when false.
   */
  fallback?: boolean, };
  ```


Run: `cargo run -p hennery-proto --bin gen`.

- [ ] **Step 4: Run them to see them pass**

- [ ] **Step 5: Revert-probes**

Each of these is caught:
- `app_server_timed_out` against `timed_out`;
- the count's increment;
- its reset on any other result;
- the threshold;
- the flag sent;
- `connection.rs` passing it through;
- the flagged path spawning no Codex;
- a retry following the count locally.

A forged flag only downgrades that same session's deletion, and a test shows it.

- [ ] **Step 6: The full checks**

Expected: all pass; **1475 tests**.

- [ ] **Step 7: Commit**

## After this plan

**What the frontend must do (plan 4):**
- show the Codex notes: `history.jsonl` and `logs_2.sqlite` are left;
- after a fallback, show "conversation copies may remain in Codex's own database", and that subagent rollouts are out of reach.

**Obligations this plan hands on:**
- **Plan 8:** record a composed home's real `~/.codex` too, since its `sessions/` link makes a forget report `symlink`.
- **The operator:** at each pin bump, run the live check that `thread/delete` still removes everything, and re-read the texts the host matches (`packaging/README.md`, "Pin bumps").

**Recorded:**
- **A "thread not found" whose rollouts are still present** retries forever. It stays visible in the host-removals list.
- **The count resets** on the collector's own pending results too: host offline, busy, no reply.
- **The `forget_codex` tests force umask 022,** so its `umask 002` run proves nothing about group-writable homes. 9d-i's tests cover that.
- **The account lookup in the directory checks has no deadline.**

**Not tested here:**
- a real Codex beyond the one live run;
- an LDAP or sssd user database;
- the SQLite copies' removal by `thread/delete`, which is Codex's contract, not verified by hennery.

**Spec amendments:** written back in this PR (ACP core §3.3, §4.10, §8).

This completes plan 9.

---

_Generated with Claude AI — please review before distribution._
