# Session workspaces implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Optional prepared Git worktrees, shared workspace membership, and archival with recoverable cleanup.

**Architecture:** The collector owns workspace identity, session membership and cleanup intent. The session host owns Git and preparation operations. A workspace can have multiple sessions; cleanup is fenced against admissions and resume, and waits for every resumable member to release it.

**Tech Stack:** Existing Rust, SQLite, host protocol, React and TypeScript.

**Spec:** `docs/specs/2026-11-18-session-workspaces.md`, based on merged proposal PR #351 and the decisions below.

## Agreed amendments

- Preserve `closed` and `parked`, including resume and their workspace membership.
- Track all workspace users, including explicit cwd selection and subdirectories. Starting, active, parked, closed and resumable failed sessions retain membership. Archived/deleted sessions release it.
- `session_spawn` shares the supplied cwd unless it explicitly requests a new worktree. Children cannot independently change their initial workspace after launch.
- New like this presents shared versus new worktree, with a configurable default; the intended user preference is new worktree.
- A new worktree starts from a selected commit without copying uncommitted changes. Show source branch, resolved commit and exclusion of local changes before launch.
- An explicit project preparation profile selects local files and an optional preparation command. Never copy all ignored files. Preparation has a visible outcome and failure blocks agent launch.
- Archive is read-only and cannot resume. It can complete while cleanup is pending or blocked. Delete retains cleanup metadata without retaining deleted conversation content.
- Preserve branches. Discarding local work requires explicit operator approval.

## Global constraints

- Conventional Commits for each commit and eventual PR title.
- Never remove externally created worktrees or arbitrary user-selected directories.
- Keep hat and external MCP root authorization valid for generated paths.
- Fail explicitly on hosts that cannot provide the requested workspace behavior.
- No auto-merge, automatic branch deletion or new claim of runtime sandboxing.

## Review focus

- Shared subdirectories, symlink aliases and sessions created outside the owner path.
- Start/resume racing cleanup, and collector/host crashes between Git and database updates.
- Dirty, untracked and ignored files; explicit discard approval must not cover later changes.
- Preparation path traversal, symlinks, command failure and accidental credential exposure.
- Archived parents/children, external control, mixed-version hosts and deletion while offline.

## Task 1: Workspace identity and membership

- [ ] Map session storage and migrations in `crates/hennery-sessions/src/store.rs` and kernel database migrations; add owner/host-scoped workspace and durable membership storage.
- [ ] Add failing store tests for multiple sessions, retained closed/parked/failed members, subdirectory identity and archive/delete release.
- [ ] Implement admission/cleanup fencing and a durable cleanup intent that survives session scrubbing. Test admission racing cleanup before proceeding.

## Task 2: Host provisioning and preparation

- [ ] Add workspace requests/results and a host capability in `crates/hennery-proto/src/frames.rs`; add a focused workspace module under `crates/hennery-host/src/`.
- [ ] Test in disposable repositories: unique branches, selected base commit, absent source changes, subdirectory mapping, ownership verification, setup success/failure and crash recovery.
- [ ] Implement provisioning and explicit file/command preparation before adapter spawn; persist ownership before destructive cleanup can be requested.
- [ ] Test cleanup refusal for retained users and local files, idempotent retries, stopped processes and unchanged branches.

## Task 3: Start, clone, delegation and external creation

- [ ] Extend `StartSessionRequest` and the delegation spawn contract with an optional workspace request; absent means share cwd for backward compatibility.
- [ ] Integrate provisioning before ACP startup in `crates/hennery-sessions/src/api.rs` and host launch. Register explicit cwd users against known workspaces.
- [ ] Add tests in `crates/hennery-testkit/tests/` for shared/new child and clone paths, preparation failure, root/hat refusals and unsupported hosts.
- [ ] Update MCP descriptions and contract fixtures; ensure archived callers/targets cannot be reactivated.

## Task 4: Archive and cleanup reconciliation

- [ ] Add explicit archive admission for closed sessions, read-only history, and cleanup status separate from lifecycle.
- [ ] Test last-member cleanup, retained shared users, offline host, dirty files, archive/delete races and delete followed by reconnect.
- [ ] Integrate durable host reconciliation and narrowly scoped explicit discard authorization; never report removal before host confirmation.

## Task 5: Settings and session UI

- [ ] Add configurable New like this default and a shared/new workspace choice to the new-session flow.
- [ ] Show the resolved base commit, excluded local changes, preparation profile/result and cleanup state; expose archive on closed sessions.
- [ ] Add behavioral UI tests for default/override, preparation failure, archived read-only state and blocked cleanup with remaining users.

## Task 6: Integration validation and review

- [ ] Run targeted Rust, host protocol, delegation and UI checks, then the repository's required checks.
- [ ] Exercise both pinned ACP adapters in disposable repositories: create, modify, park/load, close/load and archive cleanup. Record actual results and limitations.
- [ ] Review authorization and cleanup races across the full diff; update product docs and report tests and remaining risks before opening an implementation PR.

## Review status

This is a sequencing plan for review. Exact public request fields, migration ownership and fencing interfaces require a detailed implementation design before coding; they are not established contracts in this document.


## Execution record — 2026-11-18

Implemented collector registry/membership/fences, host provisioning and preparation,
operator start and clone selection, opt-in delegated worktrees, archival,
recoverable cleanup, saved preparation profiles, immutable revision preview,
inventory and explicit step-up-protected discard. Existing parked/closed resume
semantics remain in place. The implementation contract is recorded in the spec.

Observed verification:

- Session library: 192 passing tests.
- Host library: 146 passing tests before the final additional regression;
  all three workspace regression tests pass after the final ownership changes.
- Session tool contract: 11 passing tests.
- Existing defaults HTTP integration: 3 passing tests.
- New archive/shared-workspace HTTP contract, including stale-host close
  reconciliation: 1 passing test.
- Full UI suite: 2890 passing tests; subsequent workspace-form and clone tests:
  18 passing tests, including the new profile/immutable-preview test.
- Workspace-wide compilation, generated protocol check, TypeScript and whitespace
  checks passed. Independent review's reported P1 findings were addressed;
  the last review reported no remaining P1.

At the initial checkpoint, live acceptance against both pinned, authenticated
ACP adapters was pending; its completion is recorded below. The system Cargo installation has no Clippy command;
no successful Clippy run is claimed. No merge or deployment was performed.

Final upstream-base verification (the isolated PR branch contains only the
workspace implementation, with no unrelated local steering commits):

- Current main session library: 193 passing tests.
- Current main host library: 148 passing tests, including four workspace
  preparation, recovery, ownership and symlink regression tests.
- Current main UI: 2896 passing tests across 174 files.
- TypeScript, protocol generation check and compilation passed on this base.
- Preparation copying now uses directory descriptors and exclusive no-follow
  file creation rather than check-then-reopen path handling.

Live workspace acceptance completed on 2026-11-18: Claude ACP 0.107.3 and
Codex ACP 2.7.0 both passed the real-provider workspace lifecycle test, with
two provider turns per accepted run. Evidence and reproduction are recorded in
`docs/spikes/2026-11-18-workspace-live-acceptance.md`. This resolves the workspace
live-acceptance item previously marked pending above; broader release checks
and the noted Codex WebSocket fallback remain separate.

## PR checks follow-up — 2026-11-19

The PR's first Actions run failed on a collapsible-if Clippy warning and two
browser cases that still assumed New like this submitted immediately. The
recovery guard now uses a let-chain, and the browser cases explicitly submit
the workspace dialog. In both mobile/light and desktop/dark they test shared
cwd from the header and an isolated worktree from the list, committed-only
contents, unchanged source-local files, copied settings and no automatic prompt.

Observed local verification with the pinned Rust 1.98.1 tools:

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo clippy -p hennery --locked -- -D warnings`: passed.
- Built the real web UI, collector and fake adapter; both affected Chromium
  cases passed (6.6 seconds total).
- Browser-process/environment and source guards: 21 tests passed.

This supersedes the earlier note that no successful Clippy run was available.

The full Rust run subsequently exposed historical fixtures that retained the
new workspace schema while rewinding the migration version. The shared rollback
now removes workspace tables, triggers and the session column first. The deletion
walk seeds a shared workspace and checks that deleting one member preserves it.
The clone contract uses an empty request body for shared cwd and rejects unsupported
launch fields without allocating a session. Host capability expectations include
workspaces, and both preparation endpoints use the standard `ApiJson` extractor.

All 22 production workspace SQL statements are now in the owner audit. The two
membership triggers are audited from their installed SQL with row references
bound as parameters. Mutation tests reject missing owner columns, unscoped reads
and an unknown trigger's implicit insert.

Verification after these fixes:

- Full local Rust workspace: 3247 passed, 15 explicitly skipped; doctests passed.
  The run used Rust 1.98.1, `umask 0022`, an fd cap of 65536, and the pinned shell
  and proxy binaries from the development environment.
- Both Clippy checks, formatting and both generated-file checks passed.
- Owner audit after the final Clippy simplification: all four tests passed.
- Actions run 44485724103: the complete web job passed, including 2896 unit tests,
  209 browser tests (15 skipped) and two memory screen browser tests.
