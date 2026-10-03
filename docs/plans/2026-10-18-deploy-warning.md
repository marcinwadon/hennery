# The deployment warning's signal (plan 4d-B3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Kernel spec §10's warning gets a signal. When the collector shares its OS user with `hennery up`'s host child (and so with every agent of that host) while the MCP gateway holds credentials for more than one hat, hennery says so in three places, all from one verdict:
- once at start, in the collector's log (`up`'s output on a terminal), by count;
- in Settings: `deployment_warning` on `GET /api/settings` and on both `PATCH` answers, read live;
- in `hennery doctor`: check 15, asked of the running collector over its admin socket, so doctor still opens no database.

**Architecture:**
- **Gateway** (`hennery-gateway`), `store.rs`: `GatewayStore::hats_with_credentials() -> Result<BTreeSet<String>>`, exactly as the gateway lane specified it (lane L20): the distinct hats of the owner's connections with a `gw_credentials` row, owner-filtered, never decrypting, with no `cred_kind` filter (Task 1).
- **Kernel** (`hennery-kernel`):
  - `deployment.rs` (new): `Facts { beside_host, hats }`, `Isolation { NotUnderUp, OneHat, SeveralHats }` with `warns()`, `Deployment` (the flag and a hat-count closure, read on every call), `RECOMMENDATION` (Task 1).
  - `admin.rs`: the read-only `deployment` request and answer; `Admin.deployment`; the client's errors typed (`Unreachable`, `Why`, `why()`), their text unchanged (Task 4).
- **Binary** (`hennery`): a hidden `collector --beside-host` that only `UpChildren::collector` passes, on every spawn; the start line (`start_warning`); `AppState.deployment` and the admin socket wired from the gateway's store, by count (Task 2). Doctor's check 15, `doctor/isolation.rs` (Task 4).
- **Sessions** (`hennery-sessions`): `AppState.deployment`; `settings`, `update_settings` and `change_public_url` carry the verdict, a `PATCH` reading it after its step-up check and before any write (Task 2).
- **Wire** (`hennery-proto`): `SettingsResponse.deployment_warning: bool` and the same on `SettingsUpdateResponse`; the schema and TypeScript files regenerated (Task 2).
- **Specs:** kernel §4.2, §8, §10; distribution §5.1, §7; frontend §8; gateway §2 (Tasks 3, 5).

**Two PRs.** PR 1 is Tasks 1–3 (the accessor, the verdict, the start line, Settings, and their write-back); PR 2 is Tasks 4–5 (the admin question, doctor's check 15, and theirs). PR 2 touches two other owners' surfaces, the kernel's admin protocol and distribution's doctor (the security review's A8).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8, clap 4. No new crate; `Cargo.lock` does not change.

**Spec:**
- [Kernel](../specs/2026-09-26-kernel-design.md) §10: "**`hennery up` warns** at start, in Settings and in `doctor` when gateway credentials exist for more than one hat and the collector shares its OS user with the host child." Its recommendation: "whenever the gateway holds credentials for more than one hat, run the collector as a separate OS user or in a container (the Docker image), with the hosts paired to it like any remote host." §8: `GET/PATCH /api/settings`. §4.2: the admin socket.
- [Distribution](../specs/2026-09-26-distribution-design.md) §5.1: "At start `hennery up` checks whether gateway credentials exist for more than one hat while the collector shares its OS user with the host child, and warns if so." §7, check 15: "Collector isolation: gateway credentials for more than one hat while the collector shares its OS user with agents (warn, kernel spec §10)".
- [Frontend](../specs/2026-09-26-frontend-design.md) §8, Settings: "the deployment warning when the collector shares its OS user with agents while holding credentials for several hats (kernel spec §10)".
- [MCP gateway](../specs/2026-09-26-mcp-gateway-design.md) §2 (credentials read by key alone) and its "single point of compromise" paragraph; umbrella §8.4.

It builds on the doctor plans [7d-i](2026-10-13-doctor.md) and [7d-ii](2026-10-14-doctor-ii.md), whose "After this plan" both leave "**Check 15** (gateway credentials per hat): with the gateway"; on [the gateway store (8a)](2026-10-13-gateway-store.md)'s rule that lists read `gw_credentials` by key alone; on [4d-B4](2026-10-18-public-url-api.md), whose `PATCH /api/settings` paths this plan extends; and on the frontend lane's 4d split, which hands "4d-B3: the deployment warning's signal (kernel §10), for 4d-iv" ([4d-i](2026-10-18-web-manage.md)'s "After this plan"). Every anchor below was taken from `main` at `b8cf8b3` (PR #108, plan 4d-B4).

**Status:** not executed; amended after the security review. The security review of 2026-10-03 (a fresh opus reviewer on the maintainer's behalf, read-only) approved after amendments: A1–A10 required and done, O1 taken, O2 and O3 recorded (see "Decisions" and "What the review changed"). Its **scoped re-confirmation** (another fresh opus reviewer, 2026-10-03, read-only) confirmed every amendment with notes N1–N7, all applied: `Unanswered` advice moved beside `TimedOut` (N1); a catch-all for errors of no known kind (N2); `Unreachable` as the root error with today's text (N3); the closure returning a count (N4); the pre-read after the step-up check (N5); the task numbering (N6); the tests spelt out (N7). The review raised no product question.

Every code block below was built and tested in the scratch branch `scratch/4d-b3` off `b8cf8b3` (draft PR #113 runs CI on ubuntu and macOS), two commits per code task (the tests alone, then the task), one per write-back; the blocks were generated from those commits. The plan was then replayed from its own text, task by task, onto a fresh detached worktree of `b8cf8b3`: after each code task's Step 1 the tree matched that task's tests commit, and after each task the task's commit, file for file (blocks applied: Task 1 4 and 11, Task 2 5 and 26, Task 3 6, Task 4 9 and 30, Task 5 4; the second number counts the whole task, Step 1 included), and the final tree equalled the scratch's. On the scratch: `cargo fmt --all --check`, both clippy runs, `gen -- --check` and the workspace's tests are clean. `gen-view` does not exist on `b8cf8b3`.

The workspace has 1498 tests at `b8cf8b3` and 1516 after Task 4: Task 1 1505 (+7), Task 2 1510 (+5), Task 4 1516 (+6). Every revert-probe below was run on the scratch tree and caught (Task 1: 9, Task 2: 15, Task 4: 25).

## Execution status

Not executed yet.

## Scope

The specs name three places for one warning; the lanes hand this plan its pieces:
- the gateway lane's accessor (L20), which B3 adds exactly to its specification, for the gateway lane to review;
- the doctor plans' deferred check 15;
- 4d-iv's Settings banner, which needs a field to read.

That is **5 tasks** in two PRs:
1. the accessor and the verdict (pure, and the store);
2. the collector: `--beside-host`, the start line, Settings, the wire;
3. PR 1's spec write-back;
4. the admin question and doctor's check 15;
5. PR 2's spec write-back.

**In:** the accessor and its owner audit; the verdict; `--beside-host`; the start line; `deployment_warning` on every settings answer; the admin socket's `deployment`; typed admin-client errors; doctor's check 15; the specs.

**Out** (see "After this plan"):
- the Settings banner itself (4d-iv);
- detecting a host run by hand as the collector's user (G1);
- counting stdio servers' stored environment values (8e widens the accessor);
- a warning repeated while the collector runs (the spec says "at start"; Settings is live).

## Decisions this plan makes where the spec is silent

These were confirmed by the security review on the maintainer's behalf (2026-10-03, "approve after amendments") and its scoped re-confirmation. Decisions the review changed are marked "(amended)".

1. **"Shares its OS user with the host child" is told by `up`, never inferred.**
   - **Choice:** a hidden `collector --beside-host`, added by `UpChildren::collector`, the one place that builds the collector's command, on the first start and on every restart (`supervisor::Children::spawn` calls `collector(None)`). `up` never changes user: both children always run as its own. Anyone who could forge or drop the flag already controls `up`'s argv as that user, and a forged flag only adds a warning.
   - **Not inferred from `--parent-fd`** (a mechanism, not a statement), nor from a host connected over loopback: the recommended separate-user install also pairs local hosts over loopback, so that would warn exactly where the advice was taken.
   - **A collector started on its own** has no host child: no warning, the spec's literal reading (G1). A host run by hand as the collector's user is the same exposure and is not detected; check 15's ok line says so (A6), and the spec says so.
   - **Cost if wrong:** one flag.
2. **One verdict, in the kernel, by count.** (amended: A6, A7, O1)
   - **Choice:** `Facts { beside_host, hats }.isolation()` is `NotUnderUp` without the flag, `SeveralHats` beside the host with more than one hat, else `OneHat`; `warns()` is `SeveralHats` alone. The start line, Settings and doctor all read it.
   - Only a count crosses into the kernel: `Deployment`'s closure returns `usize`, and the binary maps the accessor's set with `.len()`. The log line, the admin answer and doctor's report carry a number, never a hat's id or name; tests assert it.
   - **The threshold** is the specs' "more than one hat" (G4): a hat with two credentialled connections is one hat. One hat's credentials are readable by the agents of other hats on the same user as well (O3, recorded): the specs chose "more than one hat" in four places, so it is built literally.
3. **What counts as a credential:** a `gw_credentials` row, whatever the connection's kind or status (L20). A credential that no longer works is still a secret the master key opens. OAuth grants (8f) are rows of the same table and count with no change (pinned by a test that writes one the way 8f will); stdio environment values (8e) widen the accessor later (G6).
4. **The start line.** (amended: A5)
   - **Choice:** after the gateway opens, `run_collector` logs at `warn`: "the collector runs as the OS user of hennery up's agents and holds MCP gateway credentials for N hats: any of those agents can read them all; " and the recommendation. A count that cannot be read logs "could not check …" at `warn`, never passes for a quiet start.
   - **Where it lands** (G2): `up`'s children inherit its output on a terminal, so the line is in `up`'s output; under a service each process logs to its own file, so it is in `collector.log`. Settings and doctor carry the warning anyway.
5. **Settings: one `bool`, never optional.** (amended: A4, N5)
   - **Choice:** `deployment_warning: bool` on `SettingsResponse` and `SettingsUpdateResponse` (which stays "exactly `SettingsResponse`" without `public_url_changed`). Not optional, so the compiler names all three places that build an answer.
   - **Read live** on every request. `GET` answers 500 when it cannot be read (G3). A `PATCH` reads it after its step-up check (so a stale session still hears `step_up_required`) and **before** any write, so a failed read is a 500 that changed nothing. The pre-read can be one request stale: harmless, the field drives a banner and `GET` is live. The existing post-write `contact()` reads stay as 4d-B4 recorded them.
   - **Not access control:** the field says nothing the owner cannot read in `GET /api/mcp/connections`; the banner is advice. Only 4d-iv's Settings reads it.
6. **Doctor asks the collector, over the admin socket.** (amended: A1–A3, N1–N3)
   - **Choice:** a new `AdminRequest::Deployment` answering `{beside_host, hats}`. It changes nothing: `changes_state()` lists it with `setup_url` and `list_hosts` (A1), so it is logged at `info`, and it needs no terminal. Doctor's rule is "opens no database and takes no lock" (G7): check 7 already talks to the running collector; this question is read-only, same-uid, bounded at 5 s.
   - **The client's errors carry their kind** (A2, N3): `Unreachable { why: Why }` is the error's root context, its text exactly what `hennery admin` printed before (a test pins the missing-socket text), so doctor matches on `admin::why(&err)`, never on text. `Why`: `PathTooLong`, `NoSocket`, `Stale` (refused), `Denied` (`EACCES`/`EPERM`), `OtherUser`, `TimedOut`, `Unanswered` (closed without a byte).
   - **Check 15's outcomes** (A3, N1, N2): no collector directory, or no socket (or a stale one) while no service of that directory runs: not run. While its service runs, a missing, stale, slow or silent socket warns ("the collector's service runs but its admin socket does not answer"; check 16 fails there, check 15 is a warning check). Otherwise: `TimedOut` and `Unanswered` warn ("run doctor again"); `PathTooLong` warns; `Denied` and `OtherUser` warn "run doctor as the collector's user"; an error of no known kind warns, shown cut; `refused` (an older collector) warns "restart the collector after an upgrade"; `failed` warns with the collector's message, cut; an answer to another question warns. `SeveralHats` warns with the recommendation as its fix; `OneHat` and `NotUnderUp` are ok, the latter saying what is not detected.
   - **Not tested on a real socket:** `OtherUser` needs a second uid, and `Denied` cannot be had as root (CI may be); the kernel test makes `Denied` when not root, and doctor's tests cover every kind with an injected asker. A Unix `connect` does not hang, so the connect-timeout `TimedOut` has no test; the exchange timeout has one. On Linux, a collector that dies with the request unread resets the connection: the client reads an error of no known kind, not `Unanswered`, and check 15 warns through its catch-all (it still warns; the test server reads the request before it closes, so the test means the same on both systems).

### What the review changed

| Item | Change |
|---|---|
| A1 | `Deployment` listed as read-only in `changes_state`, tested |
| A2, N3 | Typed client errors, the text unchanged |
| A3, N1, N2 | Check 15 never takes no answer for a quiet collector; a catch-all |
| A4, N5 | A `PATCH` reads the verdict after step-up, before any write |
| A5 | A start whose check fails says so |
| A6 | `NotUnderUp`, and honest ok text |
| A7, O1, N4 | Counts only, everywhere; the closure returns `usize` |
| A8, N6 | Two PRs; tasks numbered 1–5 |
| A9 | The write-back covers kernel §4.2, distribution §5.1, gateway §2 |
| A10, N7 | The tests below, each with its probe |
| O2, O3 | Recorded under "After this plan" |

## Global Constraints

- Every query names the owner (kernel §1): the new statement is in `crates/hennery-gateway/src/store.rs`, audited by `owner_filter.rs` (`GATEWAY_STATEMENTS` 23 → 24).
- What reads `gw_credentials` for a list or a count reads its key and owner only (gateway §2, plan 8a decision 3): proven under SQLite's authorizer.
- No hat id or name in the start line, the admin answer or doctor's report (A7).
- Tests that spawn the binary go through `cli.rs`'s `hennery()` (an unwritable `HOME` and `XDG_*`, no `HENNERY_DATA_DIR`, `HENNERY_HOST_DATA_DIR`, `HENNERY_MASTER_KEY`, `HENNERY_SERVICE` or `HENNERY_LOG_DIR`), and doctor runs with stand-in service managers that must not be called.
- `hennery-sessions` never depends on `hennery-gateway` (umbrella §9): the verdict reaches it as a kernel type.
- Conventional Commits, no ticket numbers; the author is the repository's gmail identity.

## Review Focus

- **A restart that forgets the flag.** The supervisor restarts the collector through `collector(None)`; it must carry `--beside-host` too. Task 2's unit test asserts both spawns.
- **The wiring forgotten.** `AppState::new` starts with `Deployment::alone()`, so a missing assignment in `run_collector` is silent; Task 2's test through the real `up` fails without it (probe Q2), and Task 4's without the admin socket's (R3).
- **A failed read that looks quiet.** `GET`, both `PATCH`es and the start line each have a test that a failed count is an error, never `false`.
- **A `PATCH` that half-happens.** A failed read after a committed `public_url` move would answer 500 to a moved collector: the read comes first (Task 2's 500 test compares the tables).
- **Doctor's no-answer cases.** Every `Why`, with and without a running service, has its line (Task 4).

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-gateway/src/store.rs` | `hats_with_credentials`, its authorizer test | 1 |
| `crates/hennery-kernel/src/deployment.rs` (new), `lib.rs` | The verdict | 1 |
| `crates/hennery-sessions/src/lib.rs`, `push.rs` | `AppState.deployment`; the settings answers | 2 |
| `crates/hennery-proto/src/rest.rs`, `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts` | `deployment_warning` | 2 |
| `crates/hennery/src/main.rs` | `--beside-host`, the wiring, the start line | 2, 4 |
| `crates/hennery-kernel/src/admin.rs` | `deployment`, `Admin.deployment`, typed client errors | 4 |
| `crates/hennery/src/admin.rs` | The CLI's answer to an answer it never asks | 4 |
| `crates/hennery/src/doctor/isolation.rs` (new), `mod.rs`, `collector.rs` | Check 15 | 4 |
| Tests: `hennery-gateway/tests/{store,owner}.rs`, `hennery-kernel/tests/deployment.rs` (new), `hennery-testkit/tests/{deployment.rs (new),owner_filter.rs,public_url.rs,push.rs}`, `hennery/tests/cli.rs` | | 1, 2, 4 |
| Tests: `hennery-kernel/tests/{admin_deployment.rs (new),admin.rs,admin_log.rs}`, `hennery/src/doctor/tests.rs` | | 4 |
| `docs/specs/…` | The write-back | 3, 5 |

All commands run from the repository root inside the dev shell (`nix develop -c …`). Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check: `cargo run -p hennery-proto --bin gen -- --check` confirms that the generated files Task 2 writes as blocks are what the generator writes. No command here changes a file.

---

### Task 1: The hats with a credential, and one verdict

**Files:**
- Create: `crates/hennery-kernel/src/deployment.rs`, `crates/hennery-kernel/tests/deployment.rs`
- Modify: `crates/hennery-gateway/src/store.rs`, `crates/hennery-kernel/src/lib.rs`, `crates/hennery-gateway/tests/store.rs`, `crates/hennery-gateway/tests/owner.rs`, `crates/hennery-testkit/tests/owner_filter.rs`

**Interfaces:**
- Produces: `GatewayStore::hats_with_credentials(&self) -> anyhow::Result<BTreeSet<String>>`; `hennery_kernel::deployment::{Facts { beside_host: bool, hats: usize }, Isolation { NotUnderUp, OneHat, SeveralHats }, Facts::isolation(self) -> Isolation, Isolation::warns(self) -> bool, Deployment::new(bool, impl Fn() -> Result<usize> + Send + Sync + 'static), Deployment::alone(), Deployment::facts(&self) -> Result<Facts>, RECOMMENDATION: &str}`.

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-gateway/tests/owner.rs`, replace:

  ```rust
      store.purge_hat(&hat).unwrap();
      assert_eq!(rows(&conn), before);
  }
  ```

  with:

  ```rust
      store.purge_hat(&hat).unwrap();
      assert_eq!(rows(&conn), before);
  }

  /// Plan 4d-B3: another owner's credentials put none of their hats among the
  /// owner's hats with credentials.
  #[test]
  fn another_owners_credentials_are_not_counted() {
      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      let _hosts = Hosts::open(&db).unwrap();
      let store = GatewayStore::open(&db).unwrap();
      let conn = rusqlite::Connection::open(&db).unwrap();
      write_other_owner(&conn, &MasterKey::from_bytes([7; 32]));
      assert!(store.hats_with_credentials().unwrap().is_empty());
  }
  ```

  In `crates/hennery-gateway/tests/store.rs`, replace:

  ```rust
      );
      assert_eq!(hennery_gateway::model::url_for_logs("not a url"), "<not a url>");
  }
  ```

  with:

  ```rust
      );
      assert_eq!(hennery_gateway::model::url_for_logs("not a url"), "<not a url>");
  }

  /// Plan 4d-B3 (kernel spec §10, lane L20): the hats whose connections hold a
  /// stored credential, each once. A connection without one counts for
  /// nothing; every kind counts, an OAuth grant (plan 8f stores it in the same
  /// table) as much as a static token; a purged hat's go with it.
  #[test]
  fn the_hats_with_credentials_are_those_a_credential_is_stored_for() {
      let w = World::new();
      let work = w.hat("Work");
      let granted = w.hat("Granted");
      let hats = || w.store.hats_with_credentials().unwrap();
      assert!(hats().is_empty());

      // Connections, but no credential yet: no hat.
      let first = w.create("first");
      let second = w.create("second");
      let mut in_work = w.new_connection("work-linear");
      in_work.hat_id = work.clone();
      let third = done(w.store.create(&in_work, NOW).unwrap()).id;
      assert!(hats().is_empty());

      // Two credentials in one hat: that hat, once.
      for connection in [&first, &second] {
          w.store.set_static_credential(connection, TOKEN, &w.key, NOW).unwrap();
      }
      assert_eq!(hats(), [w.hat.clone()].into());
      w.store.set_static_credential(&third, TOKEN, &w.key, NOW).unwrap();
      assert_eq!(hats(), [w.hat.clone(), work.clone()].into());

      // A grant of an OAuth connection, written as plan 8f will write one:
      // counted, whatever its kind.
      let sql = w.sql();
      sql.execute(
          "INSERT INTO gw_connections(id, owner_id, slug, label, url, hat_id, cred_kind, internal_network, status_at,
                                      created_at, updated_at)
           VALUES ('conn-00000000000000a1', ?1, 'granted', 'Granted', 'https://mcp.granted.example/', ?2, 'oauth_dcr',
                   0, 0, 0, 0)",
          [w.store.owner_id(), granted.as_str()],
      )
      .unwrap();
      sql.execute(
          "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, updated_at)
           VALUES ('conn-00000000000000a1', ?1, 1, x'00', 0)",
          [w.store.owner_id()],
      )
      .unwrap();
      assert_eq!(hats(), [w.hat.clone(), work.clone(), granted.clone()].into());

      // A purged hat's credentials go with it.
      w.store.purge_hat(&work).unwrap();
      assert_eq!(hats(), [w.hat.clone(), granted].into());
  }
  ```

  Create `crates/hennery-kernel/tests/deployment.rs`:

  ```rust
  //! Kernel spec §10's warning (plan 4d-B3): each verdict at its boundaries,
  //! and facts read when asked.

  use hennery_kernel::deployment::{Deployment, Facts, Isolation};
  use std::sync::Arc;
  use std::sync::atomic::{AtomicUsize, Ordering};

  fn verdict(beside_host: bool, hats: usize) -> Isolation {
      Facts { beside_host, hats }.isolation()
  }

  /// A collector `up` did not start knows of no host child sharing its user:
  /// quiet, however many hats have credentials.
  #[test]
  fn a_collector_not_under_up_is_quiet_whatever_it_holds() {
      for hats in [0, 1, 2, 7] {
          assert_eq!(verdict(false, hats), Isolation::NotUnderUp, "{hats}");
      }
      assert!(!Isolation::NotUnderUp.warns());
  }

  /// Beside `up`'s host child, credentials for one hat at most: quiet.
  #[test]
  fn beside_the_host_child_one_hat_at_most_is_quiet() {
      assert_eq!(verdict(true, 0), Isolation::OneHat);
      assert_eq!(verdict(true, 1), Isolation::OneHat);
      assert!(!Isolation::OneHat.warns());
  }

  /// Beside `up`'s host child, credentials for two hats or more: the warning.
  #[test]
  fn beside_the_host_child_two_hats_warn() {
      assert_eq!(verdict(true, 2), Isolation::SeveralHats);
      assert_eq!(verdict(true, 9), Isolation::SeveralHats);
      assert!(Isolation::SeveralHats.warns());
  }

  /// The count is read on every call, and a failed read is an error, never a
  /// quiet zero.
  #[test]
  fn the_facts_are_read_when_asked() {
      let count = Arc::new(AtomicUsize::new(1));
      let seen = count.clone();
      let deployment = Deployment::new(true, move || Ok(seen.load(Ordering::SeqCst)));
      assert_eq!(
          deployment.facts().unwrap(),
          Facts {
              beside_host: true,
              hats: 1
          }
      );
      count.store(3, Ordering::SeqCst);
      assert_eq!(deployment.facts().unwrap().isolation(), Isolation::SeveralHats);

      let broken = Deployment::new(true, || anyhow::bail!("the gateway's store is gone"));
      assert!(broken.facts().is_err());

      assert_eq!(
          Deployment::alone().facts().unwrap(),
          Facts {
              beside_host: false,
              hats: 0
          }
      );
  }
  ```

  In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
      ),
  ];

  /// The gateway store's statements (plan 8a), apart from the list above so
  /// that other lanes' changes to it stay apart from this one.
  const GATEWAY_STATEMENTS: usize = 23;

  /// Files under `crates/*/src` with SQL that the audit does not read, and
  /// why (3b-iii review, A3). Any other such file fails
  ```

  with:

  ```rust
      ),
  ];

  /// The gateway store's statements (plan 8a; plan 4d-B3's hats with a
  /// credential), apart from the list above so that other lanes' changes to
  /// it stay apart from this one.
  const GATEWAY_STATEMENTS: usize = 24;

  /// Files under `crates/*/src` with SQL that the audit does not read, and
  /// why (3b-iii review, A3). Any other such file fails
  ```


- [ ] **Step 2: Run them to see them fail**

  Run: `cargo test --locked -p hennery-kernel --test deployment` and `cargo test --locked -p hennery-gateway`
  Expected: compile errors: no module `deployment` in `hennery_kernel`; no method `hats_with_credentials`.

- [ ] **Step 3: Write the accessor and the verdict**

  In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
  use anyhow::{Context, Result, anyhow};
  use hennery_kernel::db;
  use rusqlite::{Connection, OptionalExtension, params};
  use std::path::Path;
  use std::sync::{Mutex, MutexGuard};
  use zeroize::Zeroizing;
  ```

  with:

  ```rust
  use anyhow::{Context, Result, anyhow};
  use hennery_kernel::db;
  use rusqlite::{Connection, OptionalExtension, params};
  use std::collections::BTreeSet;
  use std::path::Path;
  use std::sync::{Mutex, MutexGuard};
  use zeroize::Zeroizing;
  ```

  In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
       FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
       WHERE m.owner_id = ?1 AND h.revoked_at IS NULL AND (?2 IS NULL OR m.connection_id = ?2)
       ORDER BY m.connection_id, m.host_id";

  pub struct GatewayStore {
      conn: Mutex<Connection>,
  ```

  with:

  ```rust
       FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
       WHERE m.owner_id = ?1 AND h.revoked_at IS NULL AND (?2 IS NULL OR m.connection_id = ?2)
       ORDER BY m.connection_id, m.host_id";

  /// The hats with a stored credential (kernel spec §10, plan 4d-B3): `?1` is
  /// the owner. Whether a row is there, never what it holds; every kind
  /// counts, and so does a credential that no longer works: it is a secret
  /// the master key opens all the same.
  const SELECT_HATS_WITH_CREDENTIALS: &str = "SELECT DISTINCT c.hat_id
       FROM gw_credentials k JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
       WHERE k.owner_id = ?1";

  pub struct GatewayStore {
      conn: Mutex<Connection>,
  ```

  In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
          )?)
      }

      /// That `key` opens the newest stored credential, so a wrong key stops
      /// the start instead of sealing new rows beside ones it cannot open
      /// (plan 8a decision 8). Only the newest: a damaged older row shows when
  ```

  with:

  ```rust
          )?)
      }

      /// The hats whose connections hold a stored credential (kernel spec
      /// §10's "gateway credentials for more than one hat"; plan 4d-B3). Reads
      /// whether a credential is there, never the credential.
      pub fn hats_with_credentials(&self) -> Result<BTreeSet<String>> {
          let conn = self.conn();
          let mut stmt = conn.prepare(SELECT_HATS_WITH_CREDENTIALS)?;
          let hats = stmt.query_map([&self.owner], |r| r.get(0))?;
          Ok(hats.collect::<rusqlite::Result<_>>()?)
      }

      /// That `key` opens the newest stored credential, so a wrong key stops
      /// the start instead of sealing new rows beside ones it cannot open
      /// (plan 8a decision 8). Only the newest: a damaged older row shows when
  ```

  In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
  mod tests {
      use super::*;
      use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
      use std::collections::BTreeSet;
      use std::sync::{Arc, Mutex};

      /// Every (table, column) `sql` reads, as SQLite's authorizer reports.
  ```

  with:

  ```rust
  mod tests {
      use super::*;
      use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
      use std::sync::{Arc, Mutex};

      /// Every (table, column) `sql` reads, as SQLite's authorizer reports.
  ```

  In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
          let expected: BTreeSet<String> = ["connection_id", "owner_id"].map(String::from).into();
          assert_eq!(credentials, expected);
      }
  }
  ```

  with:

  ```rust
          let expected: BTreeSet<String> = ["connection_id", "owner_id"].map(String::from).into();
          assert_eq!(credentials, expected);
      }

      /// Plan 4d-B3 (lane L20): the hats with a credential are read from
      /// whether a row is there, by its key and owner, and nothing of the
      /// secret, as the lists are.
      #[test]
      fn the_hats_with_credentials_read_no_secret() {
          let store = GatewayStore::open_in_memory().unwrap();
          let conn = store.conn();
          let credentials: BTreeSet<String> = reads(&conn, SELECT_HATS_WITH_CREDENTIALS)
              .into_iter()
              .filter(|(table, _)| table == "gw_credentials")
              .map(|(_, column)| column)
              .collect();
          let expected: BTreeSet<String> = ["connection_id", "owner_id"].map(String::from).into();
          assert_eq!(credentials, expected);
      }
  }
  ```

  Create `crates/hennery-kernel/src/deployment.rs`:

  ```rust
  //! Whether the collector's deployment deserves kernel spec §10's warning
  //! (plan 4d-B3): "when gateway credentials exist for more than one hat and
  //! the collector shares its OS user with the host child". One verdict, read
  //! by the collector's start line, `GET /api/settings` and `hennery doctor`
  //! (over the admin socket), so no client works it out again.
  //!
  //! The collector cannot see another process's user. It knows it shares
  //! `hennery up`'s host child's because `up` says so (`--beside-host`): `up`
  //! starts both children as its own user, always. A collector started on its
  //! own does not know, and says nothing: a host run by hand as the
  //! collector's user is the same exposure, and is not detected (kernel §10's
  //! recommendation stands for it).
  //!
  //! Only counts cross into the kernel: never a hat's id or name.

  use anyhow::Result;
  use std::sync::Arc;

  /// Kernel spec §10's recommendation: the fix wherever the warning is shown.
  pub const RECOMMENDATION: &str = "run the collector as a separate OS user or in a container (the Docker image), \
                                    with the hosts paired to it like any remote host";

  /// What the verdict is drawn from.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct Facts {
      /// Started by `hennery up` beside its host child (`--beside-host`), so
      /// as the OS user of every agent of that host.
      pub beside_host: bool,
      /// How many hats have a gateway credential stored.
      pub hats: usize,
  }

  /// The verdict.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Isolation {
      /// Not started by `hennery up`: no host child shares its OS user, as far
      /// as the collector knows.
      NotUnderUp,
      /// Beside `up`'s host child, with credentials for one hat at most.
      OneHat,
      /// Beside `up`'s host child, with credentials for more than one hat:
      /// any agent of that host can read every one of them (warn).
      SeveralHats,
  }

  impl Facts {
      pub fn isolation(self) -> Isolation {
          if !self.beside_host {
              Isolation::NotUnderUp
          } else if self.hats > 1 {
              Isolation::SeveralHats
          } else {
              Isolation::OneHat
          }
      }
  }

  impl Isolation {
      /// Whether kernel spec §10's warning is due.
      pub fn warns(self) -> bool {
          self == Self::SeveralHats
      }
  }

  /// How many hats have a gateway credential stored, read when asked.
  pub type HatCount = Arc<dyn Fn() -> Result<usize> + Send + Sync>;

  /// Where the facts come from: `--beside-host`, and the gateway's store.
  #[derive(Clone)]
  pub struct Deployment {
      beside_host: bool,
      hats: HatCount,
  }

  impl Deployment {
      pub fn new(beside_host: bool, hats: impl Fn() -> Result<usize> + Send + Sync + 'static) -> Self {
          Self {
              beside_host,
              hats: Arc::new(hats),
          }
      }

      /// A collector on its own with no gateway: what a router starts with
      /// until the collector gives it its own.
      pub fn alone() -> Self {
          Self::new(false, || Ok(0))
      }

      /// The facts as they are now: the count is read on every call, since
      /// credentials come and go while the collector runs.
      pub fn facts(&self) -> Result<Facts> {
          Ok(Facts {
              beside_host: self.beside_host,
              hats: (self.hats)()?,
          })
      }
  }

  impl std::fmt::Debug for Deployment {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("Deployment")
              .field("beside_host", &self.beside_host)
              .finish_non_exhaustive()
      }
  }
  ```

  In `crates/hennery-kernel/src/lib.rs`, replace:

  ```rust
  pub mod csp;
  pub mod db;
  pub mod delivery;
  pub mod egress;
  pub mod hats;
  pub mod health;
  ```

  with:

  ```rust
  pub mod csp;
  pub mod db;
  pub mod delivery;
  pub mod deployment;
  pub mod egress;
  pub mod hats;
  pub mod health;
  ```


- [ ] **Step 4: Run the tests**

  Run: `cargo test --locked -p hennery-kernel --test deployment`, `cargo test --locked -p hennery-gateway` and `cargo test --locked -p hennery-testkit --test owner_filter`
  Expected: PASS; among them `the_hats_with_credentials_read_no_secret`, `the_hats_with_credentials_are_those_a_credential_is_stored_for`, `another_owners_credentials_are_not_counted` and the four verdict tests. The owner audit finds 24 gateway statements.

- [ ] **Step 5: Run the revert-probes**

  Each mutation is applied alone to the task's code, its test run, and the mutation undone. Every one must fail the named test.

  | Probe | Mutation in | From | To | Caught by |
  |---|---|---|---|---|
  | P1 no credential join | `store.rs` | `const SELECT_HATS_WITH_CREDENTIALS: &str = "SELECT DISTINCT c.hat_id` | `const SELECT_HATS_WITH_CREDENTIALS: &str = "SELECT DISTINCT c.hat_id` | `the_hats_with_credentials_are_those_a_credential_is_stored_for` |
  | P2 cred_kind filter | `store.rs` | `WHERE k.owner_id = ?1";` | `WHERE k.owner_id = ?1 AND c.cred_kind = 'static'";` | `the_hats_with_credentials_are_those_a_credential_is_stored_for` |
  | P3 owner filter | `store.rs` | `WHERE k.owner_id = ?1";` | `WHERE k.owner_id = ?1 OR 1";` | `another_owners_credentials_are_not_counted` |
  | P4 reads the ciphertext | `store.rs` | `const SELECT_HATS_WITH_CREDENTIALS: &str = "SELECT DISTINCT c.hat_id` | `const SELECT_HATS_WITH_CREDENTIALS: &str = "SELECT DISTINCT c.hat_id, length(k.ciphertext)` | `the_hats_with_credentials_read_no_secret` |
  | P5 not under up ignored | `deployment.rs` | `if !self.beside_host {` | `if false {` | `a_collector_not_under_up_is_quiet_whatever_it_holds` |
  | P6 one hat warns | `deployment.rs` | `} else if self.hats > 1 {` | `} else if self.hats >= 1 {` | `beside_the_host_child_one_hat_at_most_is_quiet` |
  | P7 two hats quiet | `deployment.rs` | `} else if self.hats > 1 {` | `} else if self.hats > 2 {` | `beside_the_host_child_two_hats_warn` |
  | P8 warns on one hat | `deployment.rs` | `self == Self::SeveralHats` | `self != Self::NotUnderUp` | `beside_the_host_child_one_hat_at_most_is_quiet` |
  | P9 count cached | `deployment.rs` | `hats: (self.hats)()?,` | `hats: (self.hats)().unwrap_or(0).min(1),` | `the_facts_are_read_when_asked` |

- [ ] **Step 6: Check and commit**

  Run: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked` (1505 tests).

  ```bash
  git add crates/hennery-gateway crates/hennery-kernel crates/hennery-testkit/tests/owner_filter.rs
  git commit -m "feat(gateway): the hats with a stored credential, and one verdict on the deployment"
  ```

### Task 2: The collector beside `up`'s host: the start line and Settings

**Files:**
- Create: `crates/hennery-testkit/tests/deployment.rs`
- Modify: `crates/hennery/src/main.rs`, `crates/hennery-sessions/src/lib.rs`, `crates/hennery-sessions/src/push.rs`, `crates/hennery-proto/src/rest.rs`, `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`, `crates/hennery/tests/cli.rs`, `crates/hennery-testkit/tests/public_url.rs`, `crates/hennery-testkit/tests/push.rs`

**Interfaces:**
- Consumes: Task 1's `hats_with_credentials`, `Deployment`, `Facts`, `RECOMMENDATION`.
- Produces: `CollectorArgs.beside_host` (`--beside-host`, hidden); `AppState.deployment: Deployment`; `SettingsResponse.deployment_warning` and `SettingsUpdateResponse.deployment_warning` (`bool`); `start_warning(Result<Facts>) -> Option<String>` in `main.rs`; `cli.rs`'s `credentialled_connection(listen, session, slug, hat)`.

- [ ] **Step 1: Write the failing tests**

  Create `crates/hennery-testkit/tests/deployment.rs`:

  ```rust
  //! Kernel spec §10's deployment warning in Settings (plan 4d-B3):
  //! `deployment_warning` on `GET /api/settings` and on both `PATCH` answers,
  //! drawn from the collector's `Deployment`, read before anything is written.

  use hennery_kernel::deployment::Deployment;
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::Operator;
  use hennery_kernel::secret::unix_now;
  use hennery_proto::rest::{ApiError, SettingsResponse, SettingsUpdateResponse};
  use hennery_sessions::AppState;
  use hennery_sessions::store::Store;
  use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
  use std::net::SocketAddr;
  use std::path::{Path, PathBuf};

  struct Collector {
      addr: SocketAddr,
      state: AppState,
      db: PathBuf,
  }

  impl Collector {
      /// A collector on `db`, set up at `PUBLIC_URL`, drawing its warning
      /// from `deployment`.
      async fn on(db: &Path, deployment: Deployment) -> Self {
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let mut state = AppState::new(
              Store::open(db).unwrap(),
              Hosts::open(db).unwrap(),
              Operator::open(db).unwrap(),
          );
          state.deployment = deployment;
          let token = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
          state
              .operator
              .set_up(&token, OWNER_PASSWORD, PUBLIC_URL, unix_now())
              .unwrap();
          tokio::spawn(hennery_sessions::serve(listener, state.clone()));
          Self {
              addr,
              state,
              db: db.to_path_buf(),
          }
      }

      /// A session whose last password check was `age` seconds ago.
      fn session(&self, age: i64) -> String {
          let phc = hennery_testkit::owner_phc(&self.state.operator);
          self.state
              .operator
              .open_session("test", &phc, unix_now() - age)
              .unwrap()
              .unwrap()
      }

      fn request(&self, session: &str, method: &str) -> reqwest::RequestBuilder {
          reqwest::Client::new()
              .request(method.parse().unwrap(), format!("http://{}/api/settings", self.addr))
              .header("origin", PUBLIC_URL)
              .header("cookie", format!("hennery_session={session}"))
      }

      async fn get(&self, session: &str) -> reqwest::Response {
          self.request(session, "GET").send().await.unwrap()
      }

      async fn patch(&self, session: &str, body: &str) -> reqwest::Response {
          self.request(session, "PATCH")
              .header("content-type", "application/json")
              .body(body.to_string())
              .send()
              .await
              .unwrap()
      }

      /// The owner's settings and sessions, as text: what a `PATCH` writes.
      fn dump(&self) -> Vec<String> {
          let conn = rusqlite::Connection::open(&self.db).unwrap();
          let mut rows = Vec::new();
          for table in ["owners", "settings", "auth_sessions"] {
              let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
              let columns = stmt.column_count();
              let found = stmt
                  .query_map([], |r| {
                      Ok((0..columns)
                          .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
                          .collect::<Vec<_>>()
                          .join("|"))
                  })
                  .unwrap();
              rows.extend(found.map(|row| format!("{table}: {}", row.unwrap())));
          }
          rows
      }
  }

  /// Every answer that carries the settings carries the verdict: `GET`, a
  /// `PATCH` of the contact, and a `PATCH` that moves `public_url`. True only
  /// beside `up`'s host child with credentials for more than one hat.
  #[tokio::test]
  async fn every_settings_answer_carries_the_deployment_warning() {
      for (beside_host, hats, warning) in [(true, 2, true), (true, 1, false), (false, 3, false)] {
          let case = format!("beside_host {beside_host}, {hats} hats");
          let dir = tempfile::tempdir().unwrap();
          let c = Collector::on(
              &dir.path().join("hennery.db"),
              Deployment::new(beside_host, move || Ok(hats)),
          )
          .await;
          let session = c.session(0);

          let resp = c.get(&session).await;
          assert_eq!(resp.status(), 200, "{case}");
          let got: SettingsResponse = resp.json().await.unwrap();
          assert_eq!(got.deployment_warning, warning, "GET, {case}");

          let resp = c.patch(&session, r#"{"contact":"me@example.com"}"#).await;
          assert_eq!(resp.status(), 200, "{case}");
          let got: SettingsUpdateResponse = resp.json().await.unwrap();
          assert_eq!(got.deployment_warning, warning, "PATCH contact, {case}");

          let resp = c.patch(&session, r#"{"public_url":"https://moved.example"}"#).await;
          assert_eq!(resp.status(), 200, "{case}");
          let got: SettingsUpdateResponse = resp.json().await.unwrap();
          assert!(got.public_url_changed.is_some(), "{case}");
          assert_eq!(got.deployment_warning, warning, "PATCH public_url, {case}");
      }
  }

  /// A verdict that cannot be read is a 500, never a quiet `false`, and a
  /// `PATCH` reads it before it writes anything (the review's A4): nothing
  /// changes. It is read after the step-up check, so a stale session still
  /// hears that it needs one.
  #[tokio::test]
  async fn a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(
          &dir.path().join("hennery.db"),
          Deployment::new(true, || anyhow::bail!("the gateway's store is gone")),
      )
      .await;
      let session = c.session(0);
      let stale = c.session(5 * 60);
      // Slid now, so no request below changes a session's row.
      for s in [&session, &stale] {
          assert_eq!(c.get(s).await.status(), 500);
      }
      let before = c.dump();

      let resp = c.patch(&session, r#"{"contact":"me@example.com"}"#).await;
      assert_eq!(resp.status(), 500);
      assert_eq!(c.dump(), before, "a contact was stored");

      let resp = c.patch(&session, r#"{"public_url":"https://moved.example"}"#).await;
      assert_eq!(resp.status(), 500);
      assert_eq!(c.dump(), before, "public_url moved");

      let resp = c.patch(&stale, r#"{"public_url":"https://moved.example"}"#).await;
      assert_eq!(resp.status(), 403);
      assert_eq!(resp.json::<ApiError>().await.unwrap().code, "step_up_required");
      assert_eq!(c.dump(), before);
  }
  ```

  In `crates/hennery-testkit/tests/public_url.rs`, replace:

  ```rust
          SettingsUpdateResponse {
              public_url: PUBLIC_URL.into(),
              contact: Some("you@example.com".into()),
              public_url_changed: None,
          }
      );
  ```

  with:

  ```rust
          SettingsUpdateResponse {
              public_url: PUBLIC_URL.into(),
              contact: Some("you@example.com".into()),
              deployment_warning: false,
              public_url_changed: None,
          }
      );
  ```

  In `crates/hennery-testkit/tests/public_url.rs`, replace:

  ```rust
          SettingsUpdateResponse {
              public_url: "https://moved.example".into(),
              contact: None,
              public_url_changed: Some(PublicUrlChanged {
                  sessions_ended: 2,
                  passkeys_removed: 1,
  ```

  with:

  ```rust
          SettingsUpdateResponse {
              public_url: "https://moved.example".into(),
              contact: None,
              deployment_warning: false,
              public_url_changed: Some(PublicUrlChanged {
                  sessions_ended: 2,
                  passkeys_removed: 1,
  ```

  In `crates/hennery-testkit/tests/push.rs`, replace:

  ```rust
          SettingsResponse {
              public_url: PUBLIC_URL.into(),
              contact: None,
          }
      );
      let patch = |body: serde_json::Value| client.patch(c.url("/api/settings")).json(&body).send();
  ```

  with:

  ```rust
          SettingsResponse {
              public_url: PUBLIC_URL.into(),
              contact: None,
              deployment_warning: false,
          }
      );
      let patch = |body: serde_json::Value| client.patch(c.url("/api/settings")).json(&body).send();
  ```

  In `crates/hennery/tests/cli.rs`, replace:

  ```rust
          assert!(body.contains(r#""code":"not_found""#), "{method}: {body}");
      }
      stop(&mut collector);
  }

  /// Start a collector on `data` that must fail to start: its standard error.
  ```

  with:

  ```rust
          assert!(body.contains(r#""code":"not_found""#), "{method}: {body}");
      }
      stop(&mut collector);
  }

  /// A static connection in `hat`, with a token stored for it, on the
  /// collector at `listen`.
  fn credentialled_connection(listen: &str, session: &str, slug: &str, hat: &str) {
      let body = serde_json::json!({
          "slug": slug,
          "label": slug,
          "url": format!("https://mcp.{slug}.example/mcp"),
          "hat_id": hat,
          "cred_kind": "static",
      })
      .to_string();
      let (status, created) = send_json(listen, "POST", "/api/mcp/connections", session, Some(&body)).unwrap();
      assert_eq!(status, 201, "{created}");
      let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
          .as_str()
          .unwrap()
          .to_string();
      let credential = format!("/api/mcp/connections/{id}/credential");
      let (status, _) = send_json(listen, "PUT", &credential, session, Some(r#"{"token":"tok"}"#)).unwrap();
      assert_eq!(status, 204);
  }

  /// Plan 4d-B3 (kernel spec §10, distribution spec §5.1): under `hennery up`
  /// the collector shares its OS user with the host child, so gateway
  /// credentials for two hats are warned about. Settings says so as soon as
  /// the second hat has one; the next start says so in the log, with the
  /// count and never a hat's id. A collector started on its own on the same
  /// data has no host child, and says nothing of it.
  #[test]
  fn up_warns_when_its_collector_holds_credentials_for_several_hats() {
      const LINE: &str = "holds MCP gateway credentials for 2 hats";
      let dir = scratch_dir("deploy-warning");
      let _cleanup = RemoveDir(dir.clone());
      let data = dir.join("data");
      let collector_dir = data.join("collector");
      let mut up = up_logging_to(&data, &dir.join("first.log"));
      let listen = up.listening();
      let session = sign_in(&mut up, &listen, &collector_dir);
      let warning = |listen: &str| get_json(listen, "/api/settings", &session).unwrap()["deployment_warning"].clone();
      assert_eq!(warning(&listen), false);

      let hats = get_json(&listen, "/api/hats", &session).unwrap();
      let first_hat = hats[0]["id"].as_str().unwrap().to_string();
      let (status, created) = send_json(&listen, "POST", "/api/hats", &session, Some(r#"{"name":"Work"}"#)).unwrap();
      assert_eq!(status, 201, "{created}");
      let second_hat = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
          .as_str()
          .unwrap()
          .to_string();
      credentialled_connection(&listen, &session, "linear", &first_hat);
      credentialled_connection(&listen, &session, "tracker", &first_hat);
      assert_eq!(warning(&listen), false, "two connections, one hat");
      credentialled_connection(&listen, &session, "github", &second_hat);
      assert_eq!(warning(&listen), true);
      stop(&mut up);
      let first = std::fs::read_to_string(dir.join("first.log")).unwrap();
      assert!(!first.contains("gateway credentials for"), "{first}");

      let mut again = up_logging_to(&data, &dir.join("second.log"));
      let listen = again.listening();
      assert_eq!(warning(&listen), true);
      let second = std::fs::read_to_string(dir.join("second.log")).unwrap();
      assert!(second.contains(LINE), "{second}");
      assert!(!second.contains("could not check"), "{second}");
      for hat in [&first_hat, &second_hat] {
          assert!(!second.contains(hat.as_str()), "{hat}: {second}");
      }
      stop(&mut again);

      let (mut alone, listen) = collector_on(&collector_dir, &dir.join("alone.log"));
      assert_eq!(warning(&listen), false);
      let log = std::fs::read_to_string(dir.join("alone.log")).unwrap();
      assert!(!log.contains("gateway credentials for"), "{log}");
      stop(&mut alone);
  }

  /// Start a collector on `data` that must fail to start: its standard error.
  ```


- [ ] **Step 2: Run them to see them fail**

  Run: `cargo test --locked -p hennery-testkit --test deployment`
  Expected: compile errors: no field `deployment` on `AppState`, no field `deployment_warning`.

- [ ] **Step 3: Write the flag, the start line and the settings field**

  The two generated files' blocks are what `cargo run -p hennery-proto --bin gen` writes after the `rest.rs` change.

  In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
  }

  /// `PATCH /api/settings`: absent fields stay as they are, and so does a
  ```

  with:

  ```rust
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
      /// Kernel spec §10's deployment warning: the collector runs as the OS
      /// user of `hennery up`'s host child, and so of every agent of that
      /// host, while the MCP gateway holds credentials for more than one hat.
      /// Any of those agents can read every one of them. Read when asked.
      pub deployment_warning: bool,
  }

  /// `PATCH /api/settings`: absent fields stay as they are, and so does a
  ```

  In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "PublicUrlChanged | undefined", optional)]
      pub public_url_changed: Option<PublicUrlChanged>,
  ```

  with:

  ```rust
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
      /// As in `SettingsResponse`, read before anything was changed.
      pub deployment_warning: bool,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "PublicUrlChanged | undefined", optional)]
      pub public_url_changed: Option<PublicUrlChanged>,
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  pub mod ws;

  use axum::Router;
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::Operator;
  use hennery_kernel::push::{Push, VapidKey};
  ```

  with:

  ```rust
  pub mod ws;

  use axum::Router;
  use hennery_kernel::deployment::Deployment;
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::Operator;
  use hennery_kernel::push::{Push, VapidKey};
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      pub sweep_interval: Duration,
      /// The host removals with an attempt in flight (plan 9d B7).
      pub forgets: Arc<forget::InFlight>,
  }

  impl AppState {
  ```

  with:

  ```rust
      pub sweep_interval: Duration,
      /// The host removals with an attempt in flight (plan 9d B7).
      pub forgets: Arc<forget::InFlight>,
      /// What kernel spec §10's deployment warning is drawn from (plan
      /// 4d-B3). `new` gives a collector on its own with no gateway; the
      /// collector replaces it with its own before it serves.
      pub deployment: Deployment,
  }

  impl AppState {
  ```

  In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
              push: Push::detached(),
              sweep_interval: sweep::INTERVAL,
              forgets: Arc::new(forget::InFlight::default()),
          }
      }
  }
  ```

  with:

  ```rust
              push: Push::detached(),
              sweep_interval: sweep::INTERVAL,
              forgets: Arc::new(forget::InFlight::default()),
              deployment: Deployment::alone(),
          }
      }
  }
  ```

  In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
      }
  }

  /// `GET /api/settings`: `public_url` and the push contact.
  async fn settings(State(state): State<AppState>) -> Response {
      match settings_now(&state) {
          Ok(settings) => Json(settings).into_response(),
          Err(err) => internal(err),
      }
  ```

  with:

  ```rust
      }
  }

  /// `GET /api/settings`: `public_url`, the push contact, and whether kernel
  /// spec §10's deployment warning is due, read as the request is answered.
  async fn settings(State(state): State<AppState>) -> Response {
      match deployment_warning(&state).and_then(|warning| settings_now(&state, warning)) {
          Ok(settings) => Json(settings).into_response(),
          Err(err) => internal(err),
      }
  ```

  In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
          if !session.stepped_up(unix_now()) {
              return hennery_kernel::auth::step_up_required();
          }
          return change_public_url(&state, &session, public_url, contact);
      }
      if let Some(contact) = contact {
          match state.operator.set_contact(contact) {
              Ok(Ok(())) => {}
  ```

  with:

  ```rust
          if !session.stepped_up(unix_now()) {
              return hennery_kernel::auth::step_up_required();
          }
          // Read before anything is written (plan 4d-B3, the review's A4):
          // its failure changes nothing.
          let warning = match deployment_warning(&state) {
              Ok(warning) => warning,
              Err(err) => return internal(err),
          };
          return change_public_url(&state, &session, public_url, contact, warning);
      }
      let warning = match deployment_warning(&state) {
          Ok(warning) => warning,
          Err(err) => return internal(err),
      };
      if let Some(contact) = contact {
          match state.operator.set_contact(contact) {
              Ok(Ok(())) => {}
  ```

  In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
              Err(err) => return internal(err),
          }
      }
      match settings_now(&state) {
          Ok(settings) => Json(SettingsUpdateResponse {
              public_url: settings.public_url,
              contact: settings.contact,
              public_url_changed: None,
          })
          .into_response(),
  ```

  with:

  ```rust
              Err(err) => return internal(err),
          }
      }
      match settings_now(&state, warning) {
          Ok(settings) => Json(SettingsUpdateResponse {
              public_url: settings.public_url,
              contact: settings.contact,
              deployment_warning: settings.deployment_warning,
              public_url_changed: None,
          })
          .into_response(),
  ```

  In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
  /// (the review's A1). Every session ended, the caller's too, so the cookie
  /// is cleared, as the old `public_url` set it (decision 4): this answer
  /// goes to the old origin. The move is logged as soon as it is made (A2).
  fn change_public_url(
      state: &AppState,
      session: &Authenticated,
      input: &str,
      contact: Option<Option<&str>>,
  ) -> Response {
      let caller = Some((session.session_id.as_str(), unix_now()));
      let (from, to, sessions_ended, passkeys_removed) = match state.operator.change_public_url(input, contact, caller) {
  ```

  with:

  ```rust
  /// (the review's A1). Every session ended, the caller's too, so the cookie
  /// is cleared, as the old `public_url` set it (decision 4): this answer
  /// goes to the old origin. The move is logged as soon as it is made (A2).
  /// `deployment_warning` was read before the move (plan 4d-B3).
  fn change_public_url(
      state: &AppState,
      session: &Authenticated,
      input: &str,
      contact: Option<Option<&str>>,
      deployment_warning: bool,
  ) -> Response {
      let caller = Some((session.session_id.as_str(), unix_now()));
      let (from, to, sessions_ended, passkeys_removed) = match state.operator.change_public_url(input, contact, caller) {
  ```

  In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
          Ok(contact) => Json(SettingsUpdateResponse {
              public_url: to.origin().to_string(),
              contact,
              public_url_changed: Some(PublicUrlChanged {
                  sessions_ended: sessions_ended as u64,
                  passkeys_removed: passkeys_removed as u64,
  ```

  with:

  ```rust
          Ok(contact) => Json(SettingsUpdateResponse {
              public_url: to.origin().to_string(),
              contact,
              deployment_warning,
              public_url_changed: Some(PublicUrlChanged {
                  sessions_ended: sessions_ended as u64,
                  passkeys_removed: passkeys_removed as u64,
  ```

  In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
      response
  }

  fn settings_now(state: &AppState) -> anyhow::Result<SettingsResponse> {
      let public_url = state
          .operator
          .public_url()
          .map(|url| url.origin().to_string())
          .unwrap_or_default();
      let contact = state.operator.contact()?;
      Ok(SettingsResponse { public_url, contact })
  }
  ```

  with:

  ```rust
      response
  }

  fn settings_now(state: &AppState, deployment_warning: bool) -> anyhow::Result<SettingsResponse> {
      let public_url = state
          .operator
          .public_url()
          .map(|url| url.origin().to_string())
          .unwrap_or_default();
      let contact = state.operator.contact()?;
      Ok(SettingsResponse {
          public_url,
          contact,
          deployment_warning,
      })
  }

  /// Whether kernel spec §10's warning is due (plan 4d-B3): the collector
  /// runs as the OS user of `hennery up`'s agents while the gateway holds
  /// credentials for more than one hat. A failed read is an error, never a
  /// quiet `false`.
  fn deployment_warning(state: &AppState) -> anyhow::Result<bool> {
      Ok(state.deployment.facts()?.isolation().warns())
  }
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      /// At its end-of-file `up` is gone, and the collector stops.
      #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
      parent_fd: Option<i32>,
  }

  #[derive(Args, Clone)]
  ```

  with:

  ```rust
      /// At its end-of-file `up` is gone, and the collector stops.
      #[arg(long, hide = true, value_parser = clap::value_parser!(i32).range(3..))]
      parent_fd: Option<i32>,
      /// `hennery up` only: this collector runs beside `up`'s host child, as
      /// its OS user, and so as every agent's (kernel spec §10's warning).
      #[arg(long, hide = true)]
      beside_host: bool,
  }

  #[derive(Args, Clone)]
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      // (plan 8a decision 8; `KeyUnavailable` tells that case apart).
      let keys = hennery_gateway::key::KeySource::from_env(&data_dir)?;
      let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
      // The gateway's proxy (plan 8d), `/mcp/<slug>`: bearer tokens, beside
      // the operator's routes and outside them (lane L8), sending only
      // through the kernel's egress policy, the collector's one `Egress`
  ```

  with:

  ```rust
      // (plan 8a decision 8; `KeyUnavailable` tells that case apart).
      let keys = hennery_gateway::key::KeySource::from_env(&data_dir)?;
      let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
      // Kernel spec §10 (plan 4d-B3): `up` says when this collector shares its
      // OS user with its host child; the gateway counts the hats with a
      // credential, read when asked. Only the count leaves the store.
      state.deployment = {
          let store = gateway.store.clone();
          hennery_kernel::deployment::Deployment::new(args.beside_host, move || Ok(store.hats_with_credentials()?.len()))
      };
      warn_if_shared_user(&state.deployment);
      // The gateway's proxy (plan 8d), `/mcp/<slug>`: bearer tokens, beside
      // the operator's routes and outside them (lane L8), sending only
      // through the kernel's egress policy, the collector's one `Egress`
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
              "the configured public_url is not the one setup stored, which stays in effect; \
               run `hennery admin reset-public-url` to move it"
          );
      }
  }

  ```

  with:

  ```rust
              "the configured public_url is not the one setup stored, which stays in effect; \
               run `hennery admin reset-public-url` to move it"
          );
      }
  }

  /// Kernel spec §10, at start: the collector runs as the OS user of `hennery
  /// up`'s agents while the gateway holds credentials for more than one hat.
  fn warn_if_shared_user(deployment: &hennery_kernel::deployment::Deployment) {
      if let Some(line) = start_warning(deployment.facts()) {
          tracing::warn!("{line}");
      }
  }

  /// What the collector says of kernel spec §10's warning at start, if
  /// anything: the count of hats, never one of them. A check that could not
  /// run says so, rather than passing for a quiet one.
  fn start_warning(facts: Result<hennery_kernel::deployment::Facts>) -> Option<String> {
      match facts {
          Ok(facts) if facts.isolation().warns() => Some(format!(
              "the collector runs as the OS user of hennery up's agents and holds MCP gateway credentials for {} \
               hats: any of those agents can read them all; {}",
              facts.hats,
              hennery_kernel::deployment::RECOMMENDATION
          )),
          Ok(_) => None,
          Err(err) => Some(format!(
              "could not check whether the collector shares its OS user with agents while holding credentials for \
               several hats (kernel spec §10): {err:#}"
          )),
      }
  }

  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
          }
          cmd.arg("--data-dir")
              .arg(self.data_dir.join("collector"))
              // `up` has warned about it already; the collector has no use for it.
              .env_remove(DEV_TOKEN_VAR)
              // `up` bound these addresses already; the child takes the sockets.
  ```

  with:

  ```rust
          }
          cmd.arg("--data-dir")
              .arg(self.data_dir.join("collector"))
              // Both children run as `up`'s own user, always: the collector's
              // credentials are within every agent's reach (kernel spec §10).
              .arg("--beside-host")
              // `up` has warned about it already; the collector has no use for it.
              .env_remove(DEV_TOKEN_VAR)
              // `up` bound these addresses already; the child takes the sockets.
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
      /// environment (`HENNERY_DEV_TOKEN`, left in a shell from before 3b);
      /// its host child must not inherit it, so neither can any agent that
      /// host runs.
      #[test]
      fn ups_host_child_does_not_inherit_the_operator_token() {
          let args = UpArgs {
  ```

  with:

  ```rust
      /// environment (`HENNERY_DEV_TOKEN`, left in a shell from before 3b);
      /// its host child must not inherit it, so neither can any agent that
      /// host runs.
      /// Plan 4d-B3 (kernel spec §10): every collector `up` starts, the first
      /// and every one started again after a crash, is told it runs beside the
      /// host child; the host child is not.
      #[test]
      fn ups_collector_child_is_told_it_runs_beside_the_host() {
          let args = UpArgs {
              listen: vec!["127.0.0.1:7117".into()],
              public_url: None,
              data_dir: Some("/nonexistent".into()),
              agents: Vec::new(),
              idle_timeout_secs: 0,
              workspace_roots: Vec::new(),
          };
          let children = UpChildren {
              exe: "/bin/hennery".into(),
              args: &args,
              data_dir: "/nonexistent".into(),
              host_dir: "/nonexistent/host".into(),
              collector_url: "http://127.0.0.1:7117".into(),
              collector_ws_url: "ws://127.0.0.1:7117/api/hosts/ws".into(),
              listeners: Vec::new(),
              parent: std::io::pipe().unwrap(),
          };
          let (_reader, writer) = std::io::pipe().unwrap();
          let beside = |cmd: &tokio::process::Command| cmd.as_std().get_args().filter(|a| *a == "--beside-host").count();
          assert_eq!(beside(&children.collector(None)), 1, "a collector started again");
          assert_eq!(beside(&children.collector(Some(&writer))), 1, "the first collector");
          assert_eq!(beside(&children.host(None)), 0, "the host child");
      }

      /// Plan 4d-B3: the start line names the count and the fix when the
      /// warning is due, nothing when it is not, and a check that could not
      /// run as such (the review's A5).
      #[test]
      fn the_start_line_says_what_the_verdict_is() {
          use hennery_kernel::deployment::{Facts, RECOMMENDATION};
          let line = start_warning(Ok(Facts {
              beside_host: true,
              hats: 3,
          }))
          .unwrap();
          assert!(line.contains("gateway credentials for 3 hats"), "{line}");
          assert!(line.contains(RECOMMENDATION), "{line}");
          for quiet in [(true, 1), (false, 3)] {
              let facts = Facts {
                  beside_host: quiet.0,
                  hats: quiet.1,
              };
              assert_eq!(start_warning(Ok(facts)), None, "{facts:?}");
          }
          let line = start_warning(Err(anyhow::anyhow!("the store is gone"))).unwrap();
          assert!(line.starts_with("could not check"), "{line}");
          assert!(line.contains("the store is gone"), "{line}");
      }

      #[test]
      fn ups_host_child_does_not_inherit_the_operator_token() {
          let args = UpArgs {
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
              "null"
            ]
          },
          "public_url": {
            "type": "string"
          }
        },
        "required": [
          "public_url"
        ],
        "type": "object"
      },
  ```

  with:

  ```json
              "null"
            ]
          },
          "deployment_warning": {
            "description": "Kernel spec §10's deployment warning: the collector runs as the OS\nuser of `hennery up`'s host child, and so of every agent of that\nhost, while the MCP gateway holds credentials for more than one hat.\nAny of those agents can read every one of them. Read when asked.",
            "type": "boolean"
          },
          "public_url": {
            "type": "string"
          }
        },
        "required": [
          "public_url",
          "deployment_warning"
        ],
        "type": "object"
      },
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
              "null"
            ]
          },
          "public_url": {
            "type": "string"
          },
  ```

  with:

  ```json
              "null"
            ]
          },
          "deployment_warning": {
            "description": "As in `SettingsResponse`, read before anything was changed.",
            "type": "boolean"
          },
          "public_url": {
            "type": "string"
          },
  ```

  In `schema/hennery-protocol.schema.json`, replace:

  ```json
            ]
          }
        },
        "required": [
          "public_url"
        ],
        "type": "object"
      },
      "SetupRequest": {
  ```

  with:

  ```json
            ]
          }
        },
        "required": [
          "public_url",
          "deployment_warning"
        ],
        "type": "object"
      },
      "SetupRequest": {
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```ts
   * push services may write to. Absent: they are given an `https`
   * `public_url`, and nothing for an `http` one, which Apple refuses.
   */
  contact?: string | undefined, };

  /**
   * `PATCH /api/settings`: absent fields stay as they are, and so does a
  ```

  with:

  ```ts
   * push services may write to. Absent: they are given an `https`
   * `public_url`, and nothing for an `http` one, which Apple refuses.
   */
  contact?: string | undefined, 
  /**
   * Kernel spec §10's deployment warning: the collector runs as the OS
   * user of `hennery up`'s host child, and so of every agent of that
   * host, while the MCP gateway holds credentials for more than one hat.
   * Any of those agents can read every one of them. Read when asked.
   */
  deployment_warning: boolean, };

  /**
   * `PATCH /api/settings`: absent fields stay as they are, and so does a
  ```

  In `web/src/generated/protocol.ts`, replace:

  ```ts
   * when `public_url` was changed, what the change ended. Without
   * `public_url_changed` it is exactly `SettingsResponse`.
   */
  export type SettingsUpdateResponse = { public_url: string, contact?: string | undefined, public_url_changed?: PublicUrlChanged | undefined, };

  /**
   * What a `public_url` change ended, as `hennery admin reset-public-url`
  ```

  with:

  ```ts
   * when `public_url` was changed, what the change ended. Without
   * `public_url_changed` it is exactly `SettingsResponse`.
   */
  export type SettingsUpdateResponse = { public_url: string, contact?: string | undefined, 
  /**
   * As in `SettingsResponse`, read before anything was changed.
   */
  deployment_warning: boolean, public_url_changed?: PublicUrlChanged | undefined, };

  /**
   * What a `public_url` change ended, as `hennery admin reset-public-url`
  ```


- [ ] **Step 4: Run the tests**

  Run: `cargo run --locked -p hennery-proto --bin gen -- --check`; `cargo test --locked -p hennery --bin hennery -- the_start_line ups_collector_child`; `cargo test --locked -p hennery --test cli up_warns_when`; `cargo test --locked -p hennery-testkit --test deployment --test public_url --test push`
  Expected: the generated files are current; every test passes.

- [ ] **Step 5: Run the revert-probes**

  | Probe | Mutation in | From | To | Caught by |
  |---|---|---|---|---|
  | Q1 up omits the flag | `main.rs` | `.arg("--beside-host")` | (removed) | `ups_collector_child_is_told_it_runs_beside_the_host` |
  | Q2 the deployment is not wired | `main.rs` | `state.deployment = {` | `let _unwired = {` | `up_warns_when_its_collector_holds_credentials_for_several_hats` |
  | Q3 no start line | `main.rs` | `warn_if_shared_user(&state.deployment);` | (removed) | `up_warns_when_its_collector_holds_credentials_for_several_hats` |
  | Q4 start line logged whatever | `main.rs` | `if let Some(line) = start_warning(deployment.facts()) {` | `if let Some(line) = start_warning(deployment.facts().map(\|f\| hennery_kernel::deployment::Facts { beside_host: true, ..f })) {` | `up_warns_when_its_collector_holds_credentials_for_several_hats` |
  | Q5 start line quiet on a failed read | `main.rs` | `Err(err) => Some(format!(` | `Err(_) => None,` | `panicked` |
  | Q6 start line quiet when due | `main.rs` | `Ok(facts) if facts.isolation().warns() => Some(format!(` | `Ok(facts) if facts.isolation().warns() && false => Some(format!(` | `the_start_line_says_what_the_verdict_is` |
  | Q7 GET says false | `push.rs` | `match deployment_warning(&state).and_then(\|warning\| settings_now(&state, warning)) {` | `match settings_now(&state, false) {` | `every_settings_answer_carries_the_deployment_warning` |
  | Q8 GET quiet on a failed read | `push.rs` | `match deployment_warning(&state).and_then(\|warning\| settings_now(&state, warning)) {` | `match settings_now(&state, deployment_warning(&state).unwrap_or(false)) {` | `a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing` |
  | Q9 PATCH contact says false | `push.rs` | `deployment_warning: settings.deployment_warning,` | `deployment_warning: false,` | `every_settings_answer_carries_the_deployment_warning` |
  | Q10 PATCH public_url says false | `push.rs` | `contact,` | `contact,` | `every_settings_answer_carries_the_deployment_warning` |
  | Q11 PATCH contact quiet on a failed read | `push.rs` | `let warning = match deployment_warning(&state) {` | `let warning = deployment_warning(&state).unwrap_or(false);` | `a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing` |
  | Q12 PATCH contact reads after its write | `push.rs` | `let warning = match deployment_warning(&state) {` | `if let Some(contact) = contact {` | `a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing` |
  | Q13 PATCH public_url quiet on a failed read | `push.rs` | `let warning = match deployment_warning(&state) {` | `let warning = deployment_warning(&state).unwrap_or(false);` | `a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing` |
  | Q14 PATCH public_url reads before the step-up check | `push.rs` | `if !session.stepped_up(unix_now()) {` | `if let Err(err) = deployment_warning(&state) {` | `a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing` |
  | Q15 PATCH public_url reads after its write | `push.rs` | `let warning = match deployment_warning(&state) { (the public_url path)` | `let warning = false; (and the read moved after the move)` | `a_verdict_that_cannot_be_read_is_a_500_that_changes_nothing` |

- [ ] **Step 6: Check and commit**

  Run: `cargo fmt --all --check`, both clippy runs, `cargo test --workspace --locked` (1510 tests).

  ```bash
  git add crates schema web/src/generated
  git commit -m "feat(settings): the collector beside up's host child warns of credentials for several hats"
  ```

### Task 3: PR 1's spec write-back

**Files:**
- Modify: `docs/specs/2026-09-26-kernel-design.md` (§8, §10), `docs/specs/2026-09-26-distribution-design.md` (§5.1), `docs/specs/2026-09-26-frontend-design.md` (§8), `docs/specs/2026-09-26-mcp-gateway-design.md` (§2)

- [ ] **Step 1: Write the specs' new text**

  In `docs/specs/2026-09-26-distribution-design.md`, replace:

  ```markdown

  At start `hennery up` checks whether gateway credentials exist for more than one
  hat while the collector shares its OS user with the host child, and warns if so
  (kernel spec §10).

  Before its first child, `hennery up` makes one more pipe, the parent pipe, and
  holds its write end for as long as it runs; every child it starts, restarts
  ```

  with:

  ```markdown

  At start `hennery up` checks whether gateway credentials exist for more than one
  hat while the collector shares its OS user with the host child, and warns if so
  (kernel spec §10). The check is the collector child's, told by `--beside-host`
  that it runs beside the host child (plan 4d-B3): its `warn` line is in `up`'s
  output on a terminal, and in `collector.log` under a service.

  Before its first child, `hennery up` makes one more pipe, the parent pipe, and
  holds its write end for as long as it runs; every child it starts, restarts
  ```

  In `docs/specs/2026-09-26-frontend-design.md`, replace:

  ```markdown
    §6), attachment store disk usage (ACP core §15), per-hat push policy (mute,
    include details, generic title), and the deployment warning when the collector shares its OS
    user with agents while holding credentials for several hats (kernel spec
    §10).

  **Theme:** the selected hat's colours are applied as custom properties on
  `<html>` before first render (from a small inline script reading the persisted
  ```

  with:

  ```markdown
    §6), attachment store disk usage (ACP core §15), per-hat push policy (mute,
    include details, generic title), and the deployment warning when the collector shares its OS
    user with agents while holding credentials for several hats (kernel spec
    §10; `deployment_warning` in `GET /api/settings`, which the page shows as
    advice, with the spec's recommendation).

  **Theme:** the selected hat's colours are applied as custom properties on
  `<html>` before first render (from a small inline script reading the persisted
  ```

  In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  | `GET /api/auth/passkeys`, `DELETE /api/auth/passkeys/{id}` | List passkeys (label, created, last used); remove (step-up) |
  | `POST /api/auth/step-up/password`, `…/step-up/passkey/{start,finish}` | Step-up (§3.4) |
  | `GET/DELETE /api/auth/sessions[/{id}]` | Signed-in devices (revoke: step-up) |
  | `GET/PATCH /api/settings` | `{public_url, contact}`; PATCH takes `{contact?, public_url?}`: `public_url` needs step-up and ends every session (§3.2), `contact` alone none (§6) |
  | `POST /api/hosts/pairing-codes` | Mint a pairing code (step-up) → 201 `{code, expires_at}`, or 409 `too_many_codes` (§4.1) |
  | `POST /api/hosts/enroll` | Host enrollment (code-authenticated, §4.1) → 201 `{host_id}` |
  | `GET /api/hosts`, `PATCH/DELETE /api/hosts/{id}` | List, rename/default hat, revoke (step-up) |
  ```

  with:

  ```markdown
  | `GET /api/auth/passkeys`, `DELETE /api/auth/passkeys/{id}` | List passkeys (label, created, last used); remove (step-up) |
  | `POST /api/auth/step-up/password`, `…/step-up/passkey/{start,finish}` | Step-up (§3.4) |
  | `GET/DELETE /api/auth/sessions[/{id}]` | Signed-in devices (revoke: step-up) |
  | `GET/PATCH /api/settings` | `{public_url, contact?, deployment_warning}`; PATCH takes `{contact?, public_url?}`: `public_url` needs step-up and ends every session (§3.2), `contact` alone none (§6). `deployment_warning` is §10's warning, read when asked (a `PATCH` reads it before it writes) |
  | `POST /api/hosts/pairing-codes` | Mint a pairing code (step-up) → 201 `{code, expires_at}`, or 409 `too_many_codes` (§4.1) |
  | `POST /api/hosts/enroll` | Host enrollment (code-authenticated, §4.1) → 201 `{host_id}` |
  | `GET /api/hosts`, `PATCH/DELETE /api/hosts/{id}` | List, rename/default hat, revoke (step-up) |
  ```

  In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  `HostItem`, or 404.

  *Built so far:* the auth, passkey, host, push and health routes,
  `/api/settings` (its `PATCH` taking `public_url` since plan 4d-B4),
  `POST /api/setup` and the setup page. No `/api/capabilities`,
  `PATCH /api/hosts/{id}`, hats or path rules yet.

  ```

  with:

  ```markdown
  `HostItem`, or 404.

  *Built so far:* the auth, passkey, host, push and health routes,
  `/api/settings` (its `PATCH` taking `public_url` since plan 4d-B4, and
  `deployment_warning` in every answer since plan 4d-B3),
  `POST /api/setup` and the setup page. No `/api/capabilities`,
  `PATCH /api/hosts/{id}`, hats or path rules yet.

  ```

  In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  - **`hennery up` warns** at start, in Settings and in `doctor` when gateway
    credentials exist for more than one hat and the collector shares its OS user
    with the host child.
  - The admin socket's TTY confirmation (§4.2) protects against accidents, not
    against a local process of the same user. The socket is operator-equivalent
    for any process of the collector's user: no password or session is needed to
  ```

  with:

  ```markdown
  - **`hennery up` warns** at start, in Settings and in `doctor` when gateway
    credentials exist for more than one hat and the collector shares its OS user
    with the host child.
    - **How the collector knows** (plan 4d-B3): `up` starts both children as its
      own user, always, and tells its collector so with a hidden
      `--beside-host`, at the first start and every restart. Nothing is
      inferred: a loopback host connection proves nothing (the recommended
      separate-user install pairs local hosts over loopback too). A collector
      started on its own has no host child and does not warn; a host run by
      hand as the collector's user is the same exposure and is not detected.
    - **What counts:** every hat with a row in `gw_credentials`, whatever its
      kind (gateway spec §2); two connections in one hat are one hat.
    - **Where it shows:** the collector logs it at `warn` once at start, with
      the count and this recommendation, never a hat (in `up`'s output on a
      terminal; in the collector's own log under a service); a start whose
      check fails says so. `GET /api/settings` carries `deployment_warning`,
      read on every request, for the Settings banner; a failed read is a 500,
      never a quiet `false`. The banner is advice, not access control.
  - The admin socket's TTY confirmation (§4.2) protects against accidents, not
    against a local process of the same user. The socket is operator-equivalent
    for any process of the collector's user: no password or session is needed to
  ```

  In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
    prepares the list's statements under SQLite's authorizer and finds
    `connection_id` and `owner_id` the only columns of `gw_credentials` read
    (plan 8a).
  - A connection belongs to **exactly one hat**. The same vendor in two hats is
    two connections with separate grants (umbrella §8.3). Its slug and its hat
    never change once created (moving it would carry its grant into another
  ```

  with:

  ```markdown
    prepares the list's statements under SQLite's authorizer and finds
    `connection_id` and `owner_id` the only columns of `gw_credentials` read
    (plan 8a).
  - **The hats with a credential** (`GatewayStore::hats_with_credentials`, plan
    4d-B3) are read the same way, for kernel §10's deployment warning: the
    distinct hats of the owner's connections that have a `gw_credentials` row,
    by key and owner alone, whatever the kind (an OAuth grant counts as a
    static token does). Only their count leaves the gateway. Once stdio servers
    store environment values (plan 8e), a hat with one counts too.
  - A connection belongs to **exactly one hat**. The same vendor in two hats is
    two connections with separate grants (umbrella §8.3). Its slug and its hat
    never change once created (moving it would carry its grant into another
  ```


- [ ] **Step 2: Commit**

  ```bash
  git add docs/specs
  git commit -m "docs(spec): how the collector knows it shares up's user, and where the warning shows"
  ```

PR 1 ends here.

### Task 4: Doctor's check 15, over a read-only admin question

**Files:**
- Create: `crates/hennery/src/doctor/isolation.rs`, `crates/hennery-kernel/tests/admin_deployment.rs`
- Modify: `crates/hennery-kernel/src/admin.rs`, `crates/hennery/src/admin.rs`, `crates/hennery/src/doctor/mod.rs`, `crates/hennery/src/doctor/collector.rs`, `crates/hennery/src/main.rs`, `crates/hennery/src/doctor/tests.rs`, `crates/hennery/tests/cli.rs`, `crates/hennery-kernel/tests/admin.rs`, `crates/hennery-kernel/tests/admin_log.rs`, `crates/hennery-testkit/tests/public_url.rs`

**Interfaces:**
- Consumes: Task 1's `Deployment`, `Facts`, `Isolation`, `RECOMMENDATION`; Task 2's `AppState.deployment` and `cli.rs`'s test `up_warns_when_its_collector_holds_credentials_for_several_hats`.
- Produces: `AdminRequest::Deployment`; `AdminResponse::Deployment { beside_host: bool, hats: usize }`; `Admin.deployment: Deployment`; `admin::{Unreachable, Unreachable::new(Why, String), Why, why(&anyhow::Error) -> Option<Why>}`; `doctor::isolation::{isolation, isolation_with, ask, Ask, ASK_TIMEOUT}`.

- [ ] **Step 1: Write the failing tests**

  In `crates/hennery-kernel/tests/admin.rs`, replace:

  ```rust
  use hennery_kernel::admin::{
      ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, MAX_REQUEST_BYTES, bind, read_answer, request, serve,
  };
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_kernel::secret::unix_now;
  ```

  with:

  ```rust
  use hennery_kernel::admin::{
      ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, MAX_REQUEST_BYTES, bind, read_answer, request, serve,
  };
  use hennery_kernel::deployment::Deployment;
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_kernel::secret::unix_now;
  ```

  In `crates/hennery-kernel/tests/admin.rs`, replace:

  ```rust
          hosts: hosts.clone(),
          dir: dir.to_path_buf(),
          base_url: BASE_URL.into(),
      };
      let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
      tokio::spawn(serve(socket, admin, async move {
  ```

  with:

  ```rust
          hosts: hosts.clone(),
          dir: dir.to_path_buf(),
          base_url: BASE_URL.into(),
          deployment: Deployment::alone(),
      };
      let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
      tokio::spawn(serve(socket, admin, async move {
  ```

  Create `crates/hennery-kernel/tests/admin_deployment.rs`:

  ```rust
  //! `deployment` on the admin socket (plan 4d-B3): kernel spec §10's facts,
  //! by count, for `hennery doctor`; and the client's errors, told apart by
  //! type (`admin::Why`), with the text `hennery admin` has always printed.

  use hennery_kernel::admin::{
      ADMIN_SOCKET, Admin, AdminRequest, AdminResponse, Why, bind, request, request_within, serve, why,
  };
  use hennery_kernel::deployment::Deployment;
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::Operator;
  use std::path::Path;
  use std::sync::Arc;
  use std::time::Duration;

  /// An admin socket in `dir` answering from `deployment`, until the returned
  /// sender is dropped.
  fn start(dir: &Path, deployment: Deployment) -> tokio::sync::oneshot::Sender<()> {
      let admin = Admin {
          operator: Arc::new(Operator::open_in_memory().unwrap()),
          hosts: Arc::new(Hosts::open_in_memory().unwrap()),
          dir: dir.to_path_buf(),
          base_url: "http://localhost:7117".into(),
          deployment,
      };
      let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
      tokio::spawn(serve(bind(dir).unwrap().expect("a short path"), admin, async move {
          let _ = stopped.await;
      }));
      stop
  }

  /// The facts as the collector has them, a count and no hat; a read-only
  /// question, logged at `info` like `list_hosts`.
  #[tokio::test]
  async fn deployment_answers_the_facts_by_count() {
      assert!(!AdminRequest::Deployment.changes_state());
      assert_eq!(AdminRequest::Deployment.name(), "deployment");
      let dir = tempfile::tempdir().unwrap();
      let _stop = start(dir.path(), Deployment::new(true, || Ok(2)));
      let answer = request(&dir.path().join(ADMIN_SOCKET), &AdminRequest::Deployment)
          .await
          .unwrap();
      assert_eq!(
          answer,
          AdminResponse::Deployment {
              beside_host: true,
              hats: 2
          }
      );
      assert_eq!(
          serde_json::to_string(&answer).unwrap(),
          r#"{"result":"deployment","beside_host":true,"hats":2}"#
      );
  }

  /// A count that cannot be read is `failed`, with why.
  #[tokio::test]
  async fn a_count_that_cannot_be_read_fails() {
      let dir = tempfile::tempdir().unwrap();
      let _stop = start(
          dir.path(),
          Deployment::new(true, || anyhow::bail!("the gateway's store is gone")),
      );
      let answer = request(&dir.path().join(ADMIN_SOCKET), &AdminRequest::Deployment)
          .await
          .unwrap();
      assert_eq!(
          answer,
          AdminResponse::Failed {
              message: "the gateway's store is gone".into()
          }
      );
  }

  /// Each way of getting no answer is told by its type, and its text is the
  /// one `hennery admin` printed before.
  #[tokio::test]
  async fn each_way_of_getting_no_answer_is_told_apart() {
      let dir = tempfile::tempdir().unwrap();
      let socket = dir.path().join(ADMIN_SOCKET);
      let ask = |timeout: Duration| {
          let socket = socket.clone();
          async move {
              request_within(&socket, &AdminRequest::Deployment, timeout)
                  .await
                  .unwrap_err()
          }
      };

      // No socket.
      let err = ask(Duration::from_secs(5)).await;
      assert_eq!(why(&err), Some(Why::NoSocket));
      assert_eq!(
          format!("{err}"),
          format!("connect to {} (is the collector running?)", socket.display())
      );

      // A socket nobody serves: a collector that was killed left it.
      drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
      let err = ask(Duration::from_secs(5)).await;
      assert_eq!(why(&err), Some(Why::Stale));
      std::fs::remove_file(&socket).unwrap();

      // Served, but closed without a byte of answer. The request is read
      // first: on Linux, a socket closed with bytes unread resets the
      // connection, and the client would read an error, not the end.
      let listener = tokio::net::UnixListener::bind(&socket).unwrap();
      let closing = tokio::spawn(async move {
          use tokio::io::AsyncBufReadExt;
          let (stream, _) = listener.accept().await.unwrap();
          let mut reader = tokio::io::BufReader::new(stream);
          let mut request = String::new();
          reader.read_line(&mut request).await.unwrap();
          drop(reader);
          listener
      });
      let err = ask(Duration::from_secs(5)).await;
      assert_eq!(why(&err), Some(Why::Unanswered));
      assert!(format!("{err}").starts_with("the collector closed the connection without answering"));

      // Served, and never answered.
      let listener = closing.await.unwrap();
      let holding = tokio::spawn(async move {
          let (stream, _) = listener.accept().await.unwrap();
          tokio::time::sleep(Duration::from_secs(30)).await;
          drop(stream);
      });
      let err = ask(Duration::from_millis(300)).await;
      assert_eq!(why(&err), Some(Why::TimedOut));
      assert!(format!("{err}").contains("did not answer within"), "{err}");
      holding.abort();

      // A path too long for a Unix socket.
      let long = dir.path().join("d".repeat(120)).join(ADMIN_SOCKET);
      let err = request_within(&long, &AdminRequest::Deployment, Duration::from_secs(5))
          .await
          .unwrap_err();
      assert_eq!(why(&err), Some(Why::PathTooLong));

      // A directory this user may not search: refused, unless the tests run
      // as root, whom nothing refuses (and who cannot see this case).
      // SAFETY: geteuid(2) cannot fail.
      if unsafe { libc::geteuid() } != 0 {
          use std::os::unix::fs::PermissionsExt;
          let closed = dir.path().join("closed");
          std::fs::create_dir(&closed).unwrap();
          std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
          let err = request_within(
              &closed.join(ADMIN_SOCKET),
              &AdminRequest::Deployment,
              Duration::from_secs(5),
          )
          .await
          .unwrap_err();
          std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();
          assert_eq!(why(&err), Some(Why::Denied), "{err:#}");
      }

      // Anything else has no `Why`.
      assert_eq!(why(&anyhow::anyhow!("something else")), None);
  }
  ```

  In `crates/hennery-kernel/tests/admin_log.rs`, replace:

  ```rust
          hosts,
          dir: dir.path().to_path_buf(),
          base_url: "http://localhost:7117".into(),
      };
      let (_stop, stopped) = tokio::sync::oneshot::channel::<()>();
      tokio::spawn(serve(
  ```

  with:

  ```rust
          hosts,
          dir: dir.path().to_path_buf(),
          base_url: "http://localhost:7117".into(),
          deployment: hennery_kernel::deployment::Deployment::alone(),
      };
      let (_stop, stopped) = tokio::sync::oneshot::channel::<()>();
      tokio::spawn(serve(
  ```

  In `crates/hennery-testkit/tests/public_url.rs`, replace:

  ```rust
                  hosts: admin.state.hosts.clone(),
                  dir: admin_dir.clone(),
                  base_url: PUBLIC_URL.into(),
              },
              async move {
                  let _ = stopped.await;
  ```

  with:

  ```rust
                  hosts: admin.state.hosts.clone(),
                  dir: admin_dir.clone(),
                  base_url: PUBLIC_URL.into(),
                  deployment: admin.state.deployment.clone(),
              },
              async move {
                  let _ = stopped.await;
  ```

  In `crates/hennery/src/doctor/tests.rs`, replace:

  ```rust
  use super::*;
  use crate::service::unit::{self, Role};
  use crate::service::{Manager, Platform};
  use std::cell::RefCell;
  use std::collections::BTreeMap;
  use std::os::unix::fs::{MetadataExt, PermissionsExt};
  ```

  with:

  ```rust
  use super::*;
  use crate::service::unit::{self, Role};
  use crate::service::{Manager, Platform};
  use hennery_kernel::admin::{AdminResponse, Unreachable, Why};
  use std::cell::RefCell;
  use std::collections::BTreeMap;
  use std::os::unix::fs::{MetadataExt, PermissionsExt};
  ```

  In `crates/hennery/src/doctor/tests.rs`, replace:

  ```rust
      };
      assert!(matches!(secret_files(&doctor), Finding::NotRun { number: 18, .. }));
  }
  ```

  with:

  ```rust
      };
      assert!(matches!(secret_files(&doctor), Finding::NotRun { number: 18, .. }));
  }

  /// Check 15 (plan 4d-B3) on a collector directory, answered by `answer`
  /// as the collector's admin socket would, with `cx`'s service.
  fn check15(cx: &Context, collector: &Path, answer: impl Fn() -> anyhow::Result<AdminResponse>) -> Finding {
      let before = snapshot(collector);
      let doctor = Doctor {
          cx,
          dirs: Dirs::by_contents(collector.to_path_buf(), Found::Given),
          run: &nothing,
      };
      let finding = super::isolation::isolation_with(&doctor, &|asked: &Path| {
          assert_eq!(asked, collector);
          answer()
      });
      assert_eq!(snapshot(collector), before, "doctor changed what it looked at");
      if let Finding::Checked(check) = &finding {
          assert_eq!(check.name, "collector isolation");
          if check.status != Status::Ok {
              assert!(!check.fix.is_empty(), "{check:?}");
          }
      }
      finding
  }

  fn checked15(finding: Finding) -> Check {
      match finding {
          Finding::Checked(check) => check,
          other => panic!("check 15 did not run: {other:?}"),
      }
  }

  /// A collector directory under `dir`: its database's name is enough.
  fn collector_dir(dir: &Path) -> PathBuf {
      let collector = dir.join("collector");
      std::fs::create_dir_all(&collector).unwrap();
      std::fs::write(collector.join("hennery.db"), b"").unwrap();
      collector
  }

  /// An admin client's error of kind `why`. The real client's errors are
  /// pinned in the kernel's `admin_deployment.rs`; a second user, or a CI
  /// that runs as root, cannot give every kind here.
  fn unreachable(why: Why) -> anyhow::Error {
      Unreachable::new(why, format!("{why:?}")).into()
  }

  /// Check 15: no collector directory, or no collector running, is not run;
  /// every other way of getting no answer warns, each with its own fix.
  #[test]
  fn check_15_never_takes_no_answer_for_a_quiet_collector() {
      let dir = tempfile::tempdir().unwrap();
      let fake = Fake::none();
      let cx = machine(dir.path(), Platform::Linux, &fake);

      // A host's directory: nothing to ask.
      let host = paired(&dir.path().join("host"));
      let doctor = Doctor {
          cx: &cx,
          dirs: Dirs::by_contents(host, Found::Given),
          run: &nothing,
      };
      let finding = super::isolation::isolation_with(&doctor, &|_: &Path| panic!("asked"));
      assert_eq!(
          finding,
          Finding::NotRun {
              number: 15,
              why: "no collector data directory"
          }
      );

      let collector = collector_dir(dir.path());
      for why in [Why::NoSocket, Why::Stale] {
          assert_eq!(
              check15(&cx, &collector, || Err(unreachable(why))),
              Finding::NotRun {
                  number: 15,
                  why: "the collector is not running"
              },
              "{why:?}"
          );
      }
      let cases: [(Why, &str, &str); 5] = [
          (Why::TimedOut, "did not answer in time", "run doctor again"),
          (
              Why::Unanswered,
              "closed the connection without answering",
              "run doctor again",
          ),
          (Why::PathTooLong, "path is too long", "shorter path"),
          (Why::Denied, "is not this user's", "run doctor as the collector's user"),
          (
              Why::OtherUser,
              "is not this user's",
              "run doctor as the collector's user",
          ),
      ];
      for (why, summary, fix) in cases {
          let check = checked15(check15(&cx, &collector, || Err(unreachable(why))));
          assert_eq!(check.status, Status::Warn, "{why:?}: {check:?}");
          assert!(check.summary.contains(summary), "{why:?}: {check:?}");
          assert!(check.fix.contains(fix), "{why:?}: {check:?}");
      }
      // An error of no known kind is shown, cut, and warns.
      let check = checked15(check15(&cx, &collector, || Err(anyhow::anyhow!("not a socket"))));
      assert_eq!(check.status, Status::Warn, "{check:?}");
      assert!(
          check.summary.contains("cannot ask the collector: not a socket"),
          "{check:?}"
      );
  }

  /// Check 15: while the collector's service runs, no answer from its socket
  /// warns as such, whatever the kind.
  #[test]
  fn check_15_warns_when_the_running_service_does_not_answer() {
      let dir = tempfile::tempdir().unwrap();
      let fake = systemd("active", std::process::id(), "yes");
      let cx = machine(dir.path(), Platform::Linux, &fake);
      let collector = collector_dir(dir.path());
      install(&cx, Role::Collector, &cx.exe, &collector, "/usr/bin:/bin");
      for why in [Why::NoSocket, Why::Stale, Why::TimedOut, Why::Unanswered] {
          let check = checked15(check15(&cx, &collector, || Err(unreachable(why))));
          assert_eq!(check.status, Status::Warn, "{why:?}: {check:?}");
          assert!(
              check
                  .summary
                  .contains("service runs but its admin socket does not answer"),
              "{why:?}: {check:?}"
          );
      }
  }

  /// Check 15: the collector's answers. Credentials for several hats beside
  /// up's host warn with kernel spec §10's recommendation, by count; one hat,
  /// or a collector up did not start, is ok, the latter saying what is not
  /// detected; an older collector, a failed read and an answer to another
  /// question each warn with their own fix.
  #[test]
  fn check_15_judges_the_collectors_answer() {
      let dir = tempfile::tempdir().unwrap();
      let fake = Fake::none();
      let cx = machine(dir.path(), Platform::Linux, &fake);
      let collector = collector_dir(dir.path());
      let answer = |response: AdminResponse| checked15(check15(&cx, &collector, move || Ok(response.clone())));

      let check = answer(AdminResponse::Deployment {
          beside_host: true,
          hats: 3,
      });
      assert_eq!(check.status, Status::Warn, "{check:?}");
      assert!(check.summary.contains("credentials for 3 hats"), "{check:?}");
      assert_eq!(check.fix, hennery_kernel::deployment::RECOMMENDATION);

      for hats in [0, 1] {
          let check = answer(AdminResponse::Deployment {
              beside_host: true,
              hats,
          });
          assert_eq!(check.status, Status::Ok, "{check:?}");
          assert!(check.summary.contains("at most one"), "{check:?}");
      }

      let check = answer(AdminResponse::Deployment {
          beside_host: false,
          hats: 4,
      });
      assert_eq!(check.status, Status::Ok, "{check:?}");
      assert!(check.summary.contains("not started by hennery up"), "{check:?}");
      assert!(check.summary.contains("not detected"), "{check:?}");

      let check = answer(AdminResponse::Refused {
          message: "not an admin request: unknown variant `deployment`".into(),
      });
      assert_eq!(check.status, Status::Warn, "{check:?}");
      assert!(
          check.fix.contains("restart the collector after an upgrade"),
          "{check:?}"
      );

      let check = answer(AdminResponse::Failed {
          message: "the gateway's store is gone".into(),
      });
      assert_eq!(check.status, Status::Warn, "{check:?}");
      assert!(check.summary.contains("the gateway's store is gone"), "{check:?}");
      assert!(check.fix.contains("the collector's log"), "{check:?}");

      let check = answer(AdminResponse::AlreadySetUp);
      assert_eq!(check.status, Status::Warn, "{check:?}");
      assert!(check.summary.contains("another question"), "{check:?}");
  }
  ```

  In `crates/hennery/tests/cli.rs`, replace:

  ```rust
      assert_eq!(status, 204);
  }

  /// Plan 4d-B3 (kernel spec §10, distribution spec §5.1): under `hennery up`
  /// the collector shares its OS user with the host child, so gateway
  /// credentials for two hats are warned about. Settings says so as soon as
  ```

  with:

  ```rust
      assert_eq!(status, 204);
  }

  /// `hennery doctor --data-dir <data>`'s report, with stand-ins for the
  /// service managers that only record that they ran (none may).
  fn doctor_report(dir: &std::path::Path, data: &std::path::Path) -> String {
      use std::os::unix::fs::PermissionsExt;
      let stubs = dir.join("stubs");
      std::fs::create_dir_all(&stubs).unwrap();
      for name in ["launchctl", "systemctl", "loginctl"] {
          let stub = stubs.join(name);
          std::fs::write(
              &stub,
              format!("#!/bin/sh\necho {name} >> \"{}\"\nexit 1\n", dir.join("ran").display()),
          )
          .unwrap();
          std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
      }
      let out = hennery()
          .args(["doctor", "--data-dir"])
          .arg(data)
          .env("PATH", format!("{}:/usr/bin:/bin", stubs.display()))
          .output()
          .unwrap();
      assert!(!dir.join("ran").exists(), "doctor asked a service manager");
      String::from_utf8_lossy(&out.stdout).into_owned()
  }

  /// Plan 4d-B3 (kernel spec §10, distribution spec §5.1): under `hennery up`
  /// the collector shares its OS user with the host child, so gateway
  /// credentials for two hats are warned about. Settings says so as soon as
  ```

  In `crates/hennery/tests/cli.rs`, replace:

  ```rust
      for hat in [&first_hat, &second_hat] {
          assert!(!second.contains(hat.as_str()), "{hat}: {second}");
      }
      stop(&mut again);

      let (mut alone, listen) = collector_on(&collector_dir, &dir.join("alone.log"));
      assert_eq!(warning(&listen), false);
      let log = std::fs::read_to_string(dir.join("alone.log")).unwrap();
      assert!(!log.contains("gateway credentials for"), "{log}");
      stop(&mut alone);
  }

  /// Start a collector on `data` that must fail to start: its standard error.
  ```

  with:

  ```rust
      for hat in [&first_hat, &second_hat] {
          assert!(!second.contains(hat.as_str()), "{hat}: {second}");
      }
      // Doctor asks the running collector (check 15), and says so by count.
      let report = doctor_report(&dir, &data);
      assert!(
          report.contains(
              "\nwarn 15 collector isolation: the collector runs as the OS user of hennery up's agents and holds MCP \
               gateway credentials for 2 hats"
          ),
          "{report}"
      );
      for hat in [&first_hat, &second_hat] {
          assert!(!report.contains(hat.as_str()), "{hat}: {report}");
      }
      stop(&mut again);

      let (mut alone, listen) = collector_on(&collector_dir, &dir.join("alone.log"));
      assert_eq!(warning(&listen), false);
      let log = std::fs::read_to_string(dir.join("alone.log")).unwrap();
      assert!(!log.contains("gateway credentials for"), "{log}");
      let report = doctor_report(&dir, &collector_dir);
      assert!(
          report.contains("\nok   15 collector isolation: the collector was not started by hennery up"),
          "{report}"
      );
      stop(&mut alone);
      let report = doctor_report(&dir, &collector_dir);
      assert!(report.contains("15 (the collector is not running)"), "{report}");
  }

  /// Start a collector on `data` that must fail to start: its standard error.
  ```


- [ ] **Step 2: Run them to see them fail**

  Run: `cargo test --locked -p hennery-kernel --test admin_deployment`
  Expected: compile errors: no variant `Deployment`, no field `deployment` on `Admin`, no `Why`.

- [ ] **Step 3: Write the admin question and check 15**

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
  //! - **The protocol:** one JSON `AdminRequest` on one line, at most
  //!   `MAX_REQUEST_BYTES`, within `REQUEST_TIMEOUT`; one JSON `AdminResponse`
  //!   on one line back; then the connection closes.
  //! - **One collector per data directory:** binding refuses a socket that
  //!   still answers, replaces one that refuses the connection (a collector
  //!   that was killed), and stops at any other error. The socket is removed
  //!   when its `AdminSocket` is dropped.

  use crate::hosts::Hosts;
  use crate::operator::{Operator, Reset};
  use crate::secret::unix_now;
  ```

  with:

  ```rust
  //! - **The protocol:** one JSON `AdminRequest` on one line, at most
  //!   `MAX_REQUEST_BYTES`, within `REQUEST_TIMEOUT`; one JSON `AdminResponse`
  //!   on one line back; then the connection closes.
  //! - **Read-only questions:** `deployment` answers what kernel spec §10's
  //!   warning is drawn from, by count (plan 4d-B3): `hennery doctor` asks it
  //!   rather than open the database.
  //! - **One collector per data directory:** binding refuses a socket that
  //!   still answers, replaces one that refuses the connection (a collector
  //!   that was killed), and stops at any other error. The socket is removed
  //!   when its `AdminSocket` is dropped.

  use crate::deployment::Deployment;
  use crate::hosts::Hosts;
  use crate::operator::{Operator, Reset};
  use crate::secret::unix_now;
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      ResetPublicUrl {
          public_url: String,
      },
  }

  impl AdminRequest {
      /// Whether it changes state or hands out a credential: logged at
      /// `warn`, with the peer's process id.
      pub fn changes_state(&self) -> bool {
          !matches!(self, Self::SetupUrl | Self::ListHosts)
      }

      /// The command's name, for the log: never its arguments.
  ```

  with:

  ```rust
      ResetPublicUrl {
          public_url: String,
      },
      /// What kernel spec §10's deployment warning is drawn from (plan 4d-B3),
      /// for `hennery doctor`. Reads only.
      Deployment,
  }

  impl AdminRequest {
      /// Whether it changes state or hands out a credential: logged at
      /// `warn`, with the peer's process id.
      pub fn changes_state(&self) -> bool {
          !matches!(self, Self::SetupUrl | Self::ListHosts | Self::Deployment)
      }

      /// The command's name, for the log: never its arguments.
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
              Self::ListHosts => "list_hosts",
              Self::MintPairingCode => "mint_pairing_code",
              Self::ResetPublicUrl { .. } => "reset_public_url",
          }
      }
  }
  ```

  with:

  ```rust
              Self::ListHosts => "list_hosts",
              Self::MintPairingCode => "mint_pairing_code",
              Self::ResetPublicUrl { .. } => "reset_public_url",
              Self::Deployment => "deployment",
          }
      }
  }
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
          code: String,
          expires_at: i64,
      },
      /// The request is not acceptable (why); nothing changed.
      Refused {
          message: String,
  ```

  with:

  ```rust
          code: String,
          expires_at: i64,
      },
      /// `deployment`: started by `hennery up` beside its host child, and how
      /// many hats have a gateway credential. A count, never a hat.
      Deployment {
          beside_host: bool,
          hats: usize,
      },
      /// The request is not acceptable (why); nothing changed.
      Refused {
          message: String,
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
                  .field("code", &redacted)
                  .field("expires_at", expires_at)
                  .finish(),
              Self::Refused { message } => f.debug_struct("Refused").field("message", message).finish(),
              Self::Failed { message } => f.debug_struct("Failed").field("message", message).finish(),
          }
  ```

  with:

  ```rust
                  .field("code", &redacted)
                  .field("expires_at", expires_at)
                  .finish(),
              Self::Deployment { beside_host, hats } => f
                  .debug_struct("Deployment")
                  .field("beside_host", beside_host)
                  .field("hats", hats)
                  .finish(),
              Self::Refused { message } => f.debug_struct("Refused").field("message", message).finish(),
              Self::Failed { message } => f.debug_struct("Failed").field("message", message).finish(),
          }
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      pub dir: PathBuf,
      /// The setup link's base (kernel spec §3.1).
      pub base_url: String,
  }

  /// A bound admin socket. Dropping it removes the socket file, however the
  ```

  with:

  ```rust
      pub dir: PathBuf,
      /// The setup link's base (kernel spec §3.1).
      pub base_url: String,
      /// What `deployment` answers from (plan 4d-B3): the router's own.
      pub deployment: Deployment,
  }

  /// A bound admin socket. Dropping it removes the socket file, however the
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
                  code: code.code,
                  expires_at: code.expires_at,
              }),
          AdminRequest::ResetPublicUrl { public_url } => {
              admin.operator.reset_public_url(&public_url).map(|reset| match reset {
                  Reset::Done {
  ```

  with:

  ```rust
                  code: code.code,
                  expires_at: code.expires_at,
              }),
          AdminRequest::Deployment => admin.deployment.facts().map(|facts| AdminResponse::Deployment {
              beside_host: facts.beside_host,
              hats: facts.hats,
          }),
          AdminRequest::ResetPublicUrl { public_url } => {
              admin.operator.reset_public_url(&public_url).map(|reset| match reset {
                  Reset::Done {
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      outcome.unwrap_or_else(|err| AdminResponse::Failed {
          message: format!("{err:#}"),
      })
  }

  /// How long the client waits for the whole exchange: connecting, sending
  ```

  with:

  ```rust
      outcome.unwrap_or_else(|err| AdminResponse::Failed {
          message: format!("{err:#}"),
      })
  }

  /// Why the client got no answer, as the context of its error (plan 4d-B3,
  /// the review's A2): a caller tells the cases apart by this type, never by
  /// the text. Its text is the message `hennery admin` has always printed.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Unreachable {
      pub why: Why,
      message: String,
  }

  /// The cases of `Unreachable`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Why {
      /// The socket's path is too long for a Unix socket: the collector runs
      /// without one.
      PathTooLong,
      /// No socket there.
      NoSocket,
      /// A socket nothing serves (refused): left by a collector that stopped.
      Stale,
      /// Not this user's to reach (`EACCES`, `EPERM`).
      Denied,
      /// Served by another user.
      OtherUser,
      /// No connection, or no whole answer, in time.
      TimedOut,
      /// Closed without a byte of answer.
      Unanswered,
  }

  impl Unreachable {
      pub fn new(why: Why, message: String) -> Self {
          Self { why, message }
      }
  }

  impl std::fmt::Display for Unreachable {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.write_str(&self.message)
      }
  }

  impl std::error::Error for Unreachable {}

  /// The `Why` of an admin client's error, if it has one.
  pub fn why(err: &anyhow::Error) -> Option<Why> {
      err.downcast_ref::<Unreachable>().map(|u| u.why)
  }

  /// How long the client waits for the whole exchange: connecting, sending
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      match tokio::time::timeout_at(deadline, exchange).await {
          Ok(answer) => answer,
          // Part of the request, or all of it, may have arrived.
          Err(_) => bail!(
              "the collector at {} did not answer within {timeout:?}; the command's outcome is unknown",
              socket.display()
          ),
      }
  }

  ```

  with:

  ```rust
      match tokio::time::timeout_at(deadline, exchange).await {
          Ok(answer) => answer,
          // Part of the request, or all of it, may have arrived.
          Err(_) => Err(Unreachable::new(
              Why::TimedOut,
              format!(
                  "the collector at {} did not answer within {timeout:?}; the command's outcome is unknown",
                  socket.display()
              ),
          )
          .into()),
      }
  }

  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      timeout: Duration,
  ) -> Result<tokio::net::UnixStream> {
      if socket_path_too_long(socket) {
          bail!(
              "{} is {} bytes long, longer than a Unix socket path can be ({} bytes at most): \
               the collector serving that data directory has no admin socket; \
               move the data directory to a shorter path",
              socket.display(),
              socket.as_os_str().len(),
              max_socket_path_bytes()
          );
      }
      let stream = match tokio::time::timeout_at(deadline, tokio::net::UnixStream::connect(socket)).await {
          Ok(connected) => {
              connected.with_context(|| format!("connect to {} (is the collector running?)", socket.display()))?
          }
          Err(_) => bail!(
              "connecting to {} took over {timeout:?}; nothing was sent",
              socket.display()
          ),
      };
      // The other way round too: a password is sent only to a collector of
      // this user's.
  ```

  with:

  ```rust
      timeout: Duration,
  ) -> Result<tokio::net::UnixStream> {
      if socket_path_too_long(socket) {
          return Err(Unreachable::new(
              Why::PathTooLong,
              format!(
                  "{} is {} bytes long, longer than a Unix socket path can be ({} bytes at most): \
                   the collector serving that data directory has no admin socket; \
                   move the data directory to a shorter path",
                  socket.display(),
                  socket.as_os_str().len(),
                  max_socket_path_bytes()
              ),
          )
          .into());
      }
      let stream = match tokio::time::timeout_at(deadline, tokio::net::UnixStream::connect(socket)).await {
          Ok(Ok(stream)) => stream,
          Ok(Err(err)) => {
              let message = format!("connect to {} (is the collector running?)", socket.display());
              let why = match err.kind() {
                  std::io::ErrorKind::NotFound => Some(Why::NoSocket),
                  std::io::ErrorKind::ConnectionRefused => Some(Why::Stale),
                  std::io::ErrorKind::PermissionDenied => Some(Why::Denied),
                  _ => None,
              };
              return Err(match why {
                  Some(why) => anyhow::Error::new(err).context(Unreachable::new(why, message)),
                  None => anyhow::Error::new(err).context(message),
              });
          }
          Err(_) => {
              return Err(Unreachable::new(
                  Why::TimedOut,
                  format!(
                      "connecting to {} took over {timeout:?}; nothing was sent",
                      socket.display()
                  ),
              )
              .into());
          }
      };
      // The other way round too: a password is sent only to a collector of
      // this user's.
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      // SAFETY: geteuid(2) cannot fail.
      let own = unsafe { libc::geteuid() };
      if server != own {
          bail!(
              "{} is served by user id {server}, not by this user ({own}); nothing was sent",
              socket.display()
          );
      }
      Ok(stream)
  }
  ```

  with:

  ```rust
      // SAFETY: geteuid(2) cannot fail.
      let own = unsafe { libc::geteuid() };
      if server != own {
          return Err(Unreachable::new(
              Why::OtherUser,
              format!(
                  "{} is served by user id {server}, not by this user ({own}); nothing was sent",
                  socket.display()
              ),
          )
          .into());
      }
      Ok(stream)
  }
  ```

  In `crates/hennery-kernel/src/admin.rs`, replace:

  ```rust
      // Serde's own message for an empty input ("EOF while parsing a value")
      // reads like a parse bug, not like what happened.
      if read == 0 {
          bail!(
              "the collector closed the connection without answering (it may have stopped); \
               the command's outcome is unknown"
          );
      }
      serde_json::from_str(&answer).context("the collector's answer")
  }
  ```

  with:

  ```rust
      // Serde's own message for an empty input ("EOF while parsing a value")
      // reads like a parse bug, not like what happened.
      if read == 0 {
          return Err(Unreachable::new(
              Why::Unanswered,
              "the collector closed the connection without answering (it may have stopped); \
               the command's outcome is unknown"
                  .to_string(),
          )
          .into());
      }
      serde_json::from_str(&answer).context("the collector's answer")
  }
  ```

  In `crates/hennery/src/admin.rs`, replace:

  ```rust
          }
          AdminResponse::Refused { message } => bail!("refused: {message}"),
          AdminResponse::Failed { message } => bail!("the collector failed: {message}"),
      }
      Ok(())
  }
  ```

  with:

  ```rust
          }
          AdminResponse::Refused { message } => bail!("refused: {message}"),
          AdminResponse::Failed { message } => bail!("the collector failed: {message}"),
          // Doctor's question (plan 4d-B3): no command here asks it.
          AdminResponse::Deployment { .. } => bail!("the collector answered another command"),
      }
      Ok(())
  }
  ```

  In `crates/hennery/src/doctor/collector.rs`, replace:

  ```rust
  const NO_ANSWER: &str = "the collector did not answer (check 7)";

  /// `text` cut to `MAX_QUOTE` characters: words that come from the network.
  fn quoted(text: &str) -> String {
      let mut out: String = text.chars().take(MAX_QUOTE).collect();
      if text.chars().count() > MAX_QUOTE {
          out.push('…');
  ```

  with:

  ```rust
  const NO_ANSWER: &str = "the collector did not answer (check 7)";

  /// `text` cut to `MAX_QUOTE` characters: words that come from the network.
  pub(super) fn quoted(text: &str) -> String {
      let mut out: String = text.chars().take(MAX_QUOTE).collect();
      if text.chars().count() > MAX_QUOTE {
          out.push('…');
  ```

  In `crates/hennery/src/doctor/collector.rs`, replace:

  ```rust

  /// `future` run to its end on a runtime of its own, on a thread of its own:
  /// doctor runs inside `main`'s runtime, which cannot be blocked on.
  fn block_on<T: Send + 'static>(future: impl std::future::Future<Output = T> + Send + 'static) -> T {
      std::thread::spawn(move || {
          tokio::runtime::Builder::new_current_thread()
              .enable_all()
  ```

  with:

  ```rust

  /// `future` run to its end on a runtime of its own, on a thread of its own:
  /// doctor runs inside `main`'s runtime, which cannot be blocked on.
  pub(super) fn block_on<T: Send + 'static>(future: impl std::future::Future<Output = T> + Send + 'static) -> T {
      std::thread::spawn(move || {
          tokio::runtime::Builder::new_current_thread()
              .enable_all()
  ```

  In `crates/hennery/src/doctor/collector.rs`, replace:

  ```rust
  }

  /// Whether the one installed service runs the collector in `dir`.
  fn served(doctor: &Doctor, dir: &std::path::Path) -> bool {
      use crate::service::unit::Role;
      let cx = doctor.cx;
      let [role] = cx.installed()[..] else {
  ```

  with:

  ```rust
  }

  /// Whether the one installed service runs the collector in `dir`.
  pub(super) fn served(doctor: &Doctor, dir: &std::path::Path) -> bool {
      use crate::service::unit::Role;
      let cx = doctor.cx;
      let [role] = cx.installed()[..] else {
  ```

  Create `crates/hennery/src/doctor/isolation.rs`:

  ```rust
  //! Check 15 (distribution spec §7, kernel spec §10; plan 4d-B3): the
  //! collector holds gateway credentials for more than one hat while it
  //! shares its OS user with the agents of `hennery up`'s host. Doctor opens
  //! no database: it asks the running collector over its admin socket
  //! (`deployment`, read-only), and prints only the verdict and a count.
  //! Every way of getting no answer is told by its type (`admin::Why`); none
  //! passes for a quiet collector.

  use super::collector::{block_on, quoted, served};
  use super::{Doctor, Finding, Verdict};
  use hennery_kernel::admin::{ADMIN_SOCKET, AdminRequest, AdminResponse, Why};
  use hennery_kernel::deployment::{Facts, Isolation, RECOMMENDATION};
  use std::path::Path;
  use std::time::Duration;

  /// How long the collector has to answer.
  pub const ASK_TIMEOUT: Duration = Duration::from_secs(5);

  const NAME: &str = "collector isolation";

  /// Asks the collector in a data directory its `deployment`.
  pub type Ask<'a> = &'a dyn Fn(&Path) -> anyhow::Result<AdminResponse>;

  /// Ask the collector in `collector` over its admin socket, bounded.
  pub fn ask(collector: &Path) -> anyhow::Result<AdminResponse> {
      let socket = collector.join(ADMIN_SOCKET);
      block_on(
          async move { hennery_kernel::admin::request_within(&socket, &AdminRequest::Deployment, ASK_TIMEOUT).await },
      )
  }

  /// Check 15.
  pub fn isolation(doctor: &Doctor) -> Finding {
      isolation_with(doctor, &ask)
  }

  /// Check 15, asking with `ask`.
  pub fn isolation_with(doctor: &Doctor, ask: Ask) -> Finding {
      let Some(collector) = &doctor.dirs.collector else {
          return Finding::NotRun {
              number: 15,
              why: "no collector data directory",
          };
      };
      let mut verdict = Verdict::default();
      match ask(collector) {
          Ok(AdminResponse::Deployment { beside_host, hats }) => match (Facts { beside_host, hats }).isolation() {
              Isolation::SeveralHats => verdict.warn(
                  format!(
                      "the collector runs as the OS user of hennery up's agents and holds MCP gateway credentials \
                       for {hats} hats: any of those agents can read them all"
                  ),
                  RECOMMENDATION,
              ),
              Isolation::OneHat => verdict.ok(format!(
                  "the collector runs beside hennery up's host, with MCP gateway credentials for {hats} hat(s): at \
                   most one"
              )),
              Isolation::NotUnderUp => verdict.ok(
                  "the collector was not started by hennery up; a host run by hand as its OS user is not detected \
                   (kernel spec §10)",
              ),
          },
          Ok(AdminResponse::Refused { .. }) => verdict.warn(
              "the collector does not know this question",
              "restart the collector after an upgrade",
          ),
          Ok(AdminResponse::Failed { message }) => verdict.warn(
              format!("the collector could not answer: {}", quoted(&message)),
              "see the collector's log",
          ),
          Ok(_) => verdict.warn(
              "the collector gave an answer to another question",
              "restart the collector after an upgrade",
          ),
          Err(err) => {
              let running = served(doctor, collector);
              match hennery_kernel::admin::why(&err) {
                  Some(Why::NoSocket | Why::Stale) if !running => {
                      return Finding::NotRun {
                          number: 15,
                          why: "the collector is not running",
                      };
                  }
                  Some(Why::NoSocket | Why::Stale | Why::TimedOut | Why::Unanswered) if running => verdict.warn(
                      "the collector's service runs but its admin socket does not answer",
                      "check the collector's log, and run doctor again",
                  ),
                  Some(Why::TimedOut) => verdict.warn(
                      "the collector did not answer in time",
                      "run doctor again once the collector is idle",
                  ),
                  Some(Why::Unanswered) => verdict.warn(
                      "the collector closed the connection without answering",
                      "run doctor again",
                  ),
                  Some(Why::PathTooLong) => verdict.warn(
                      "cannot ask the collector: its admin socket's path is too long",
                      "move the data directory to a shorter path",
                  ),
                  Some(Why::Denied | Why::OtherUser) => verdict.warn(
                      "the collector's admin socket is not this user's",
                      "run doctor as the collector's user",
                  ),
                  Some(Why::NoSocket | Why::Stale) | None => verdict.warn(
                      format!("cannot ask the collector: {}", quoted(&format!("{err:#}"))),
                      "check the collector's log",
                  ),
              }
          }
      }
      Finding::Checked(verdict.check(15, NAME))
  }
  ```

  In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
  //! and a working hennery, one check at a time, each `ok`, `warn` or `fail`
  //! with a one-sentence fix. Doctor only reads (decision 2): it never
  //! repairs, pairs, logs in or installs; it opens no database and takes no
  //! lock a host, an install or `up` takes. The one program of the user's it
  //! runs is the login shell, to compare its PATH with the service's (check
  //! 5), as `service install` does. Everything it reads of the machine comes
  //! through the service commands' `Context` and a `Runner`, which the tests
  ```

  with:

  ```rust
  //! and a working hennery, one check at a time, each `ok`, `warn` or `fail`
  //! with a one-sentence fix. Doctor only reads (decision 2): it never
  //! repairs, pairs, logs in or installs; it opens no database and takes no
  //! lock a host, an install or `up` takes; what only the collector's database
  //! holds, it asks the running collector over its admin socket, read-only
  //! (check 15, plan 4d-B3). The one program of the user's it
  //! runs is the login shell, to compare its PATH with the service's (check
  //! 5), as `service install` does. Everything it reads of the machine comes
  //! through the service commands' `Context` and a `Runner`, which the tests
  ```

  In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
  pub(crate) mod dirs;
  mod disk;
  mod env;
  mod platform;
  mod process;
  mod runtime;
  ```

  with:

  ```rust
  pub(crate) mod dirs;
  mod disk;
  mod env;
  mod isolation;
  mod platform;
  mod process;
  mod runtime;
  ```

  In `crates/hennery/src/doctor/mod.rs`, replace:

  ```rust
          runtime::adapter_set(doctor),
          agents::bundled_and_terminal(doctor),
          service::host_directory(doctor),
          collector::listeners(doctor),
          runtime::cli_overrides(doctor),
          secrets::secret_files(doctor),
  ```

  with:

  ```rust
          runtime::adapter_set(doctor),
          agents::bundled_and_terminal(doctor),
          service::host_directory(doctor),
          isolation::isolation(doctor),
          collector::listeners(doctor),
          runtime::cli_overrides(doctor),
          secrets::secret_files(doctor),
  ```

  In `crates/hennery/src/main.rs`, replace:

  ```rust
              hosts: state.hosts.clone(),
              dir: data_dir.clone(),
              base_url,
          };
          tokio::spawn(hennery_kernel::admin::serve(
              socket,
  ```

  with:

  ```rust
              hosts: state.hosts.clone(),
              dir: data_dir.clone(),
              base_url,
              deployment: state.deployment.clone(),
          };
          tokio::spawn(hennery_kernel::admin::serve(
              socket,
  ```


- [ ] **Step 4: Run the tests**

  Run: `cargo test --locked -p hennery-kernel --test admin_deployment --test admin --test admin_log`; `cargo test --locked -p hennery --bin hennery doctor`; `cargo test --locked -p hennery --test cli up_warns_when`
  Expected: PASS. The `hennery admin` tests in `cli.rs` (a path too long, a collector that never answers) pass unchanged: the messages did not change.

- [ ] **Step 5: Run the revert-probes**

  | Probe | Mutation in | From | To | Caught by |
  |---|---|---|---|---|
  | R1 deployment changes state | `admin.rs` | `Self::SetupUrl \| Self::ListHosts \| Self::Deployment)` | `Self::SetupUrl \| Self::ListHosts)` | `deployment_answers_the_facts_by_count` |
  | R2 wrong count answered | `admin.rs` | `hats: facts.hats,` | `hats: facts.hats.min(1),` | `deployment_answers_the_facts_by_count` |
  | R3 admin socket not given the deployment | `main.rs` | `deployment: state.deployment.clone(),` | `deployment: hennery_kernel::deployment::Deployment::alone(),` | `up_warns_when_its_collector_holds_credentials_for_several_hats` |
  | R4 check 15 not in the report | `mod.rs` | `isolation::isolation(doctor),` | (removed) | `up_warns_when_its_collector_holds_credentials_for_several_hats` |
  | R5 no socket untyped | `admin.rs` | `std::io::ErrorKind::NotFound => Some(Why::NoSocket),` | (removed) | `each_way_of_getting_no_answer_is_told_apart` |
  | R6 stale untyped | `admin.rs` | `std::io::ErrorKind::ConnectionRefused => Some(Why::Stale),` | (removed) | `each_way_of_getting_no_answer_is_told_apart` |
  | R7 denied untyped | `admin.rs` | `std::io::ErrorKind::PermissionDenied => Some(Why::Denied),` | (removed) | `each_way_of_getting_no_answer_is_told_apart` |
  | R8 exchange timeout untyped | `admin.rs` | `Err(_) => Err(Unreachable::new(` | `Err(_) => Err(Unreachable::new(` | `each_way_of_getting_no_answer_is_told_apart` |
  | R9 unanswered untyped | `admin.rs` | `Why::Unanswered,` | `Why::TimedOut,` | `each_way_of_getting_no_answer_is_told_apart` |
  | R10 path too long untyped | `admin.rs` | `Why::PathTooLong,` | `Why::NoSocket,` | `each_way_of_getting_no_answer_is_told_apart` |
  | R11 no-socket text changed | `admin.rs` | `let message = format!("connect to {} (is the collector running?)", socket.display());` | `let message = format!("no collector at {}", socket.display());` | `each_way_of_getting_no_answer_is_told_apart` |
  | D1 no collector dir | `isolation.rs` | `why: "no collector data directory",` | `why: "nothing to ask",` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D2 not running warns | `isolation.rs` | `Some(Why::NoSocket \| Why::Stale) if !running => {` | `Some(Why::NoSocket \| Why::Stale) if false => {` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D3 running service silent ignored | `isolation.rs` | `Some(Why::NoSocket \| Why::Stale \| Why::TimedOut \| Why::Unanswered) if running => verdict.warn(` | `Some(Why::NoSocket \| Why::Stale \| Why::TimedOut \| Why::Unanswered) if false => verdict.warn(` | `check_15_warns_when_the_running_service_does_not_answer` |
  | D4 timeout ok | `isolation.rs` | `Some(Why::TimedOut) => verdict.warn(` | `Some(Why::TimedOut) => verdict.ok("the collector did not answer in time"),` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D5 unanswered ok | `isolation.rs` | `Some(Why::Unanswered) => verdict.warn(` | `Some(Why::Unanswered) => verdict.ok("the collector closed the connection without answering"),` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D6 path too long not run | `isolation.rs` | `Some(Why::PathTooLong) => verdict.warn(` | `Some(Why::PathTooLong) => return Finding::NotRun { number: 15, why: "the collector is not running" },` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D7 another user's socket advice | `isolation.rs` | `"run doctor as the collector's user",` | `"run doctor again",` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D8 unknown error not run | `isolation.rs` | `Some(Why::NoSocket \| Why::Stale) \| None => verdict.warn(` | `Some(Why::NoSocket \| Why::Stale) \| None => verdict.ok(format!("{}", 0)), #[allow(unreachable_patterns)] _ => verdict.warn(` | `check_15_never_takes_no_answer_for_a_quiet_collector` |
  | D9 refused ok | `isolation.rs` | `Ok(AdminResponse::Refused { .. }) => verdict.warn(` | `Ok(AdminResponse::Refused { .. }) => verdict.ok("the collector does not know this question"),` | `check_15_judges_the_collectors_answer` |
  | D10 failed hidden | `isolation.rs` | `format!("the collector could not answer: {}", quoted(&message)),` | `format!("the collector could not answer{}", &message[..0]),` | `check_15_judges_the_collectors_answer` |
  | D11 other answer ok | `isolation.rs` | `Ok(_) => verdict.warn(` | `Ok(_) => verdict.ok("the collector gave an answer to another question"),` | `check_15_judges_the_collectors_answer` |
  | D12 several hats ok | `isolation.rs` | `Isolation::SeveralHats => verdict.warn(` | `Isolation::SeveralHats => verdict.fail(` | `check_15_judges_the_collectors_answer` |
  | D13 one hat warns | `isolation.rs` | `Isolation::OneHat => verdict.ok(format!(` | `Isolation::OneHat => verdict.warn("x", format!(` | `check_15_judges_the_collectors_answer` |
  | D14 not under up claims too much | `isolation.rs` | `"the collector was not started by hennery up; a host run by hand as its OS user is not detected \` | `"the collector was not started by hennery up",` | `check_15_judges_the_collectors_answer` |

- [ ] **Step 6: Check and commit**

  Run: `cargo fmt --all --check`, both clippy runs, `cargo test --workspace --locked` (1516 tests).

  ```bash
  git add crates
  git commit -m "feat(doctor): check 15, the collector's isolation, over a read-only admin question"
  ```

### Task 5: PR 2's spec write-back

**Files:**
- Modify: `docs/specs/2026-09-26-kernel-design.md` (§4.2, §10), `docs/specs/2026-09-26-distribution-design.md` (§7)

- [ ] **Step 1: Write the specs' new text**

  In `docs/specs/2026-09-26-distribution-design.md`, replace:

  ```markdown
  | 12 | Adapter set: installed set differs from the one pinned by this binary (warn, with `hennery host adapters update`) |
  | 13 | Bundled vs terminal CLI: the pinned bundled `claude`/`codex` version compared with the one on the user's PATH; warn on a large gap (sessions resumed from the terminal may meet an unexpected format) |
  | 14 | Single instance: no other host process holds `host.lock` in this data directory |
  | 15 | Collector isolation: gateway credentials for more than one hat while the collector shares its OS user with agents (warn, kernel spec §10) |
  | 16 | Collector listeners: every configured address bound; `public_url` reaches one of them or a reverse proxy (warn, kernel spec §7) |
  | 17 | CLI overrides (`--use-cli`): the operator's CLI version against the pinned one (warn on a gap, §13) |
  | 18 | The collector's secret files, `vapid.key` and `master.key`, judged by their metadata alone, never read, as the collector takes them and in its order: a regular file, not a symlink; no group or other bits; owned by the collector directory's owner and readable by it; for `master.key`, one name only; 32 bytes. Each way the collector would refuse one fails, with its fix. A file not made yet is fine, but once `hennery.db` exists a missing one warns: a new `vapid.key` breaks every push subscription, and a missing `master.key` stops a collector with stored gateway credentials. One that can't be looked at warns (run doctor as the collector's user). `hennery backup` carries both (kernel §9) |
  ```

  with:

  ```markdown
  | 12 | Adapter set: installed set differs from the one pinned by this binary (warn, with `hennery host adapters update`) |
  | 13 | Bundled vs terminal CLI: the pinned bundled `claude`/`codex` version compared with the one on the user's PATH; warn on a large gap (sessions resumed from the terminal may meet an unexpected format) |
  | 14 | Single instance: no other host process holds `host.lock` in this data directory |
  | 15 | Collector isolation: gateway credentials for more than one hat while the collector shares its OS user with agents (warn, kernel spec §10). Asked of the running collector over its admin socket (kernel §4.2's `deployment`), never read from the database: not run without a collector directory or with no collector running; any other way of getting no answer warns with its fix (a running service whose socket is silent, a socket not this user's: run doctor as the collector's user, an older collector: restart it); ok names what is not detected (a host run by hand as the collector's user) |
  | 16 | Collector listeners: every configured address bound; `public_url` reaches one of them or a reverse proxy (warn, kernel spec §7) |
  | 17 | CLI overrides (`--use-cli`): the operator's CLI version against the pinned one (warn on a gap, §13) |
  | 18 | The collector's secret files, `vapid.key` and `master.key`, judged by their metadata alone, never read, as the collector takes them and in its order: a regular file, not a symlink; no group or other bits; owned by the collector directory's owner and readable by it; for `master.key`, one name only; 32 bytes. Each way the collector would refuse one fails, with its fix. A file not made yet is fine, but once `hennery.db` exists a missing one warns: a new `vapid.key` breaks every push subscription, and a missing `master.key` stops a collector with stored gateway credentials. One that can't be looked at warns (run doctor as the collector's user). `hennery backup` carries both (kernel §9) |
  ```

  In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  `hennery backup` / `hennery restore`. **State-changing commands** (backup,
  restore, password reset, `public_url` reset, pairing-code minting) require a
  typed `yes` on a terminal; without a terminal the CLI refuses and sends
  nothing. `setup-url` and `hosts` need none. There is no `--yes`: scripts mint
  pairing codes over the HTTP API with step-up. A new password is typed twice on
  the terminal with echo off, never as an argument (§2). The confirmation is
  enforced by the CLI: it stops accidental and non-interactive use, not a process
  ```

  with:

  ```markdown
  `hennery backup` / `hennery restore`. **State-changing commands** (backup,
  restore, password reset, `public_url` reset, pairing-code minting) require a
  typed `yes` on a terminal; without a terminal the CLI refuses and sends
  nothing. `setup-url` and `hosts` need none, nor does `deployment`, the
  read-only question `hennery doctor` asks (below). There is no `--yes`: scripts mint
  pairing codes over the HTTP API with step-up. A new password is typed twice on
  the terminal with echo off, never as an argument (§2). The confirmation is
  enforced by the CLI: it stops accidental and non-interactive use, not a process
  ```

  In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
    the owner spotting it. The CLI says so before it asks for the password, and
    reports the sessions ended and passkeys removed. A later change-password
    route must not be the recovery: it keeps the passkeys.
  - **`reset-public-url <url>`** refuses before setup, replaces the stored value
    with no restart, ends every session, and removes the passkeys only when the
    host name changes (§3.2). The CLI warns about passkeys before it asks, and
  ```

  with:

  ```markdown
    the owner spotting it. The CLI says so before it asks for the password, and
    reports the sessions ended and passkeys removed. A later change-password
    route must not be the recovery: it keeps the passkeys.
  - **`deployment`** (plan 4d-B3) answers `{beside_host, hats}`: what §10's
    warning is drawn from, a count and never a hat. It changes nothing and is
    logged at `info`. `hennery doctor` asks it (check 15, within 5 s) instead of
    opening the database; the client's errors carry their kind (no socket, a
    stale one, not this user's, served by another user, a timeout, closed
    unanswered, a path too long), so doctor never reads their text.
  - **`reset-public-url <url>`** refuses before setup, replaces the stored value
    with no restart, ends every session, and removes the passkeys only when the
    host name changes (§3.2). The CLI warns about passkeys before it asks, and
  ```

  In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
      terminal; in the collector's own log under a service); a start whose
      check fails says so. `GET /api/settings` carries `deployment_warning`,
      read on every request, for the Settings banner; a failed read is a 500,
      never a quiet `false`. The banner is advice, not access control.
  - The admin socket's TTY confirmation (§4.2) protects against accidents, not
    against a local process of the same user. The socket is operator-equivalent
    for any process of the collector's user: no password or session is needed to
  ```

  with:

  ```markdown
      terminal; in the collector's own log under a service); a start whose
      check fails says so. `GET /api/settings` carries `deployment_warning`,
      read on every request, for the Settings banner; a failed read is a 500,
      never a quiet `false`. The banner is advice, not access control. `hennery
      doctor` asks the running collector over the admin socket (§4.2's
      `deployment`; distribution §7, check 15).
  - The admin socket's TTY confirmation (§4.2) protects against accidents, not
    against a local process of the same user. The socket is operator-equivalent
    for any process of the collector's user: no password or session is needed to
  ```


- [ ] **Step 2: Commit**

  ```bash
  git add docs/specs
  git commit -m "docs(spec): doctor's check 15 asks the collector over its admin socket"
  ```

## Final checks

`cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p hennery --locked -- -D warnings`; `cargo test --workspace --locked` (1516); `cargo run --locked -p hennery-proto --bin gen -- --check`. Before each push: `git log --format='%ae' origin/main..HEAD` shows only the gmail address.

## After this plan

- **4d-iv (the Settings page):** read `deployment_warning` from `GET /api/settings` (and keep it from a `PATCH` answer); show the warning with kernel §10's recommendation as advice. It is no access control and hides nothing. Its text locators follow the fleet rule (`{ exact: true }` or a role).
- **The gateway lane (8e):** when stdio servers store environment values, `hats_with_credentials` counts a hat with one too (gateway §2 says so); the owner audit's `GATEWAY_STATEMENTS` moves with it. 8f's OAuth grants are counted already (a test writes one as 8f will). Review the accessor as specified (lane L20).
- **Merge friction:** `GATEWAY_STATEMENTS`, `hennery-proto` and the generated files are hotspots: whoever merges second renumbers and regenerates.
- **The distribution lane:** check 15 now exists; doctor's report gains its line on every collector directory.
- **O2 (recorded):** doctor could see part of G1 without a protocol change: a paired host in the examined directories whose collector URL is loopback, while that collector answers on this uid with two hats or more, could warn.
- **O3 (recorded):** credentials for one hat are readable by every agent on that user too; the specs warn only from two hats.
- **Debt kept from 4d-B4:** the `PATCH` paths still read `contact()` after a committed write.

---

_Generated with Claude AI — please review before distribution._
