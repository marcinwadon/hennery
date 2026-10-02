# `public_url` over the HTTP API (plan 4d-B4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Settings can move the collector. `PATCH /api/settings {public_url}` does exactly what `hennery admin reset-public-url` does (kernel spec §3.2, §4.2), behind a fresh step-up (§3.4):
- one operator function for both paths, in one transaction: the URL checked as setup checks it, the stored row and the cached origin replaced, every signed-in session ended (the caller's too) and so every stream and push subscription, every passkey removed when the host name changes and only then, every passkey ceremony ended;
- the step-up is conditional on the body: a request naming `public_url` needs it, checked before anything is read or written; one with only `contact` needs none (kernel §6);
- the write re-checks the caller's session, live and stepped up, as its first statement (the review's A1);
- the answer reports what ended, as the admin socket does, and clears the cookie as the old `public_url` set it;
- the change is logged at `warn` with both origins.

**Architecture:**
- **Kernel** (`hennery-kernel`), `operator.rs`:
  - `Operator::change_public_url(input, contact, caller) -> PublicUrlChange`: the body `reset_public_url` had, plus the contact (`UPDATE owners SET contact`) in the same transaction and, for a caller, the session re-check as its first statement. The transaction is `IMMEDIATE`, since its first statement may now be a read.
  - `PublicUrlChange { Done { from, to, sessions_ended, passkeys_removed }, NotSetUp, Invalid, SignedOut, StepUpRequired }`: `from` and `to` read and written under the lock.
  - `reset_public_url(input)` calls it with no contact and no caller, and maps it back to `Reset`: the admin socket's path and answers are unchanged.
- **Sessions** (`hennery-sessions`), `push.rs`: `update_settings` takes the request's `Authenticated`; a body naming `public_url` is stepped up or refused, then handed to the operator with its caller; `with_cleared_cookie`; `settings_now` returns the settings rather than a response.
- **Wire** (`hennery-proto`): `SettingsUpdateRequest` gains `public_url?`; the `PATCH` answers `SettingsUpdateResponse { public_url, contact?, public_url_changed?: PublicUrlChanged { sessions_ended, passkeys_removed } }`, which without `public_url_changed` is exactly `SettingsResponse`. The schema and TypeScript files are regenerated.
- **Specs:** kernel §3.2, §3.4, §4.2, §6, §8; umbrella §7.5; frontend §8 (Task 3).
- **Tests:** the kernel's `tests/operator.rs` (the transaction, the caller's re-check); the testkit's new `tests/public_url.rs` (over HTTP, and the API against the admin socket on a copy of one database) and `tests/public_url_log.rs` (the log line, in a binary of its own); `owner_filter.rs` (`operator.rs`: 29 statements); `push.rs` (its `public_url` refusal goes).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crate: the testkit gains `tracing-subscriber`, already in the workspace, as a dev-dependency, so `Cargo.lock` changes by one line (Task 2, one `cargo build --workspace` without `--locked`).

**Spec:** [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md):
- §3.2: "Changing `public_url` … Settings says so before saving, and the change requires step-up (§3.4)."
- §3.3: state-changing browser requests carry `public_url`'s `Origin`.
- §3.4: "changing `public_url`" needs step-up.
- §4.2: `reset-public-url <url>` "replaces the stored value with no restart, ends every session, and removes the passkeys only when the host name changes".
- §6: "`PATCH /api/settings {contact}` sets it (no step-up)"; "a password or `public_url` reset ends every session and so every subscription".
- §8: `GET/PATCH /api/settings`, and its "Built so far": "a `public_url` in `PATCH /api/settings` is 422 `invalid_body`".

It builds on [passkeys 3c](2026-10-05-passkeys.md), whose "After this plan" hands it on: "`PATCH /api/settings` (`public_url`, step-up, kernel §3.4) must remove the passkeys and clear the ceremonies as `reset_public_url` does (decision 9), in the same transaction as the change." It also builds on [3b-ii](2026-10-03-operator-auth-2.md) decision 12 (the cached origin, and its recorded `Origin` race) and [push 10a](2026-10-14-push.md) decision 4 (subscriptions end with their sessions). Every anchor below was taken from `main` at `ecc50cd`, which merged PR #93 (the gateway's differential tests). The plan was first built on `27fa020` (PR #82, plan 4b); `27fa020..ecc50cd` shares no file with it, and every block applied unchanged.

**Status:** not executed; amended after the security review. The security review of 2026-10-02 (an opus subagent, binding on the maintainer's behalf) approved after amendments: A1–A4 required and done, O2 taken, O1 declined; its scoped re-confirmation of the same day confirmed every amendment, the A2 deviation included, with two notes (the 401's `Secure` flag read from the cache, harmless; the password-login race named in kernel §3.2, taken).

Every code block below was built and tested in a scratch copy of `ecc50cd`, two commits per task (the tests alone, then the task), and generated from those commits. The plan was then replayed from its own text, task by task, onto a fresh copy of `ecc50cd`: after each Step 1 the tree matched the scratch's tests-only commit, and after each task the scratch's task commit, byte for byte (the same git tree), `Cargo.lock` and the generated files included. The workspace's tests went from 1399 to 1412 (Task 1: 1401; Task 2: 1412). Every revert-probe below was run on the scratch's tree (26 in all, P5 both ways) and every one was caught. The scratch ran on ubuntu and macOS in the scratch CI PR (#97).

After the scoped re-confirmation, the re-check on `ecc50cd` added one test and seven probes, and no line of code: `a_step_up_lapsed_while_its_body_arrives_changes_nothing` (the handler's answer to the write's `StepUpRequired`, which had been listed as not tested), and P20–P26 (the operator's `NotSetUp` and contact `Invalid`, that answer, `deny_unknown_fields`, and `reset_public_url`'s mapping back to `Reset`), so that every outcome of `PublicUrlChange` has its own test and probe.

## Execution status

Not executed yet.

## Scope

The hand-off from 3c, the pieces of kernel §3.2 and §3.4 that name Settings, and the frontend lane's need (plan 4d's Settings page saves `public_url`). That is **3 tasks**:
1. the operator: one function for both paths, the contact in its transaction, the caller re-checked;
2. the HTTP API: the wire types, the conditional step-up, the answer and the cookie, and the tests over HTTP;
3. the spec write-back.

**In:**
- `PATCH /api/settings` taking `public_url`, with or without `contact`;
- parity with `hennery admin reset-public-url`, proven by a differential test on a copy of one database;
- the old origin refused afterwards, the new one served;
- the specs' statements updated.

**Out** (see "After this plan"):
- the Settings page itself, its warning and its confirmation (plan 4d's frontend);
- a two-phase move confirmed from the new origin, and a server-side reachability probe (decision 5);
- closing 3b-ii D12's race for every handler (decision 7);
- OAuth registrations, which a move invalidates (the gateway's OAuth plan, 8e).

**Where the hand-offs land:**

| Hand-off | Here |
|---|---|
| 3c: `PATCH /api/settings` removes the passkeys and clears the ceremonies as `reset_public_url` does, in the same transaction | Decision 1; Tasks 1, 2 |
| Kernel §3.4: changing `public_url` needs step-up | Decision 2; Task 2 |
| Kernel §6: `contact` alone needs no step-up | Decision 2; Task 2 |
| 10a decision 4: subscriptions end with their sessions | Decision 6; Task 2 |
| 3b-ii D12: the cached-origin race | Decision 7: recorded |

## Decisions this plan makes where the spec is silent

These were confirmed by a stronger-model security review on the maintainer's behalf (2026-10-02, "approve after amendments"). Each gives the choice, the alternatives, and the cost if it is wrong. Decisions the review changed are marked "(amended after the security review of 2026-10-02)". Items marked **(amendment)** depart from explicit spec text and are written back into it in Task 3.

**What the review changed:**
- A1: the write re-checks the caller's session, live and stepped up, as the transaction's first statement, as a passkey registration's write does (3c A2). A session revoked or a password reset after the request's checks changes nothing: 401 `unauthenticated` or 403 `step_up_required` (decision 2; Tasks 1, 2).
- A2: the operator returns the origin left and the origin taken, both read under the lock; the handler logs the move at `warn` as soon as it is made, before it builds the answer, and takes the cookie's `Secure` flag from the origin left (decisions 4, 5; Tasks 1, 2). The review asked for `Reset::Done` to carry them; they are on a type of its own, `PublicUrlChange::Done`, so the admin path's `Reset` and its tests stay as they are (re-confirmed).
- A3: tests: a real escaped key (it had been decoded on its way into the test, so the row was a copy of another), a duplicate key `null` first, `null` beside a contact, the caller's re-check at the kernel level, and a same-origin `PATCH` (Tasks 1, 2).
- A4: the spec write-back (Task 3), including the frontend's duties under decision 5.
- Optional, taken: O2, the 401 of A1 clears the cookie. Not taken: O1, the admin path logging both origins; kernel §4.2 logs only a command's name, never its arguments.

1. **One operator function, one transaction, for both paths.** (amended after the security review of 2026-10-02: A1, A2)
   - **Choice:** `Operator::change_public_url(input, contact, caller)` holds what `reset_public_url` did; `reset_public_url(input)` calls it with `None, None`.
     - Checked before the lock, nothing written on a refusal: the URL by `PublicUrl::parse` (setup's rule) and the contact by `push::contact_problem`. Either refused is `Invalid`.
     - In one `IMMEDIATE` transaction: the caller's re-check (A1, decision 2), the contact when the body names one (`Some(None)` clears it), the `settings` row, every `auth_sessions` row, every `push_subscriptions` row, and the passkeys when the RP id changes (3c decision 9). After the commit, still under the connection's lock: the cached `public_url`, and every ceremony cleared. Then the session-end generation is bumped, so every stream re-checks its session and ends.
     - `PublicUrlChange::Done { from, to, … }`: `from` is read under the lock, where the passkey comparison reads it (A2).
     - The audit's floor for `operator.rs` goes from 27 to 29 statements (the contact `UPDATE`, the re-check `SELECT`).
   - **Proof of parity:** `the_api_and_the_admin_socket_leave_the_same_state` seeds one database (a contact, two sessions, a subscription each, two passkeys), copies it with `VACUUM INTO`, starts a login ceremony on each copy, moves one over HTTP and the other over the admin socket, and compares every row of `owners`, `settings`, `password_credentials`, `auth_sessions`, `push_subscriptions` and `passkeys`, the cached origin, the ceremonies, the counts and the announced ending. Once for a host change and once for a port change, so both of 3c decision 9's branches are compared.
   - **Alternatives:**
     - **A copy of the reset's body in the handler:** two paths that drift; 3c's hand-off forbids it.
     - **A body naming both fields refused,** or the contact set in a second transaction: simpler, but a body could then change one and fail the other.
   - **Cost if wrong:** one function; the admin socket's answers do not change.
2. **Step-up is conditional on the body, and checked twice.** (amended after the security review of 2026-10-02: A1, A3)
   - **Choice:**
     - A body whose `public_url` is present and not `null` needs a fresh step-up. `update_settings` checks `Authenticated::stepped_up` before anything is read or written, so a stale session's `contact` beside it is not stored either (plan 5d decision 2's pattern for `PATCH /api/sessions/{id}` with a hat).
     - A body without one, or with `"public_url": null`, takes the contact path as before: no step-up (kernel §6).
     - **In the write** (A1): the transaction's first statement reads the caller's session, live (`expires_at > now`) and stepped up (`last_step_up_at` within `STEP_UP_SECS`, `Authenticated::stepped_up`'s window). Gone: `SignedOut`, 401 `unauthenticated`, cookie cleared (O2). Stepped down: `StepUpRequired`, 403 `step_up_required`. Nothing is written, cleared or announced. Moving `public_url` is the step-up action with the largest reach, and `admin reset-password` is the recovery after a compromise: a `PATCH` already in flight, or one whose body is sent slowly, must not land after it.
   - **The verdict cannot differ by decoding** (fleet rules): the step-up decision and the change read the same deserialised struct. Pinned from a stale session, the full tables compared before and after each: `public_url` is the key (403); a duplicate key, `null` first or not, another case and a stray space are 422 `invalid_body`, as is a number; a BOM is 400 `invalid_body`; `"public_url": null` beside a contact stores the contact with no step-up.
   - **Alternatives:** step-up on every `PATCH`: a contact change would need a password check, against kernel §6. A separate route (`PUT /api/settings/public_url`): one more route for one field; the frontend's Settings form already sends `PATCH`.
   - **Cost if wrong:** a condition in one handler.
3. **The same origin again is a change like any other.**
   - **Choice:** parity with the admin socket, whose reset to the same URL ends every session too. Every session, subscription and ceremony ends; the passkeys stay (the host name is the same). The frontend sends `public_url` only when the owner changed it (`the_same_origin_again_ends_every_session_and_keeps_the_passkeys`).
   - **Alternatives:** an unchanged origin as a no-op: a comparison under the lock or a second path, and an API that differs from the admin socket's.
   - **Cost if wrong:** one comparison.
4. **The answer: 200 with what ended, and the cookie cleared as the old `public_url` set it.** (amended after the security review of 2026-10-02: A2, O2)
   - **Choice:**
     - 200 `SettingsUpdateResponse { public_url, contact?, public_url_changed: { sessions_ended, passkeys_removed } }`: the new origin, which the page goes to, and what `hennery admin reset-public-url` reports. A `PATCH` without `public_url` answers the same type without `public_url_changed`: byte for byte the `SettingsResponse` it answered before.
     - **The cookie is cleared** (`Max-Age=0`), since the caller's session has ended. The answer goes to the old origin, which set the cookie, so it is `Secure` as the *old* `public_url` was: from `https` to loopback `http` it is `Secure`, the other way not. Cookies ignore ports, so a same-host move clears it for the new port too.
     - **Exactly one `Set-Cookie`:** `require_operator` re-sends a slid session's cookie only when the handler set none and the session is still live; here neither holds (`a_change_ends_every_session_stream_subscription_and_ceremony` uses a session the request slides).
     - The cookie is cleared on a 500 after the commit too, and on A1's 401 (O2). A 403 `step_up_required` keeps it: the session lives.
   - **Alternatives:** 204 (no counts to report); 200 keeping the cookie (it names a dead session).
   - **Cost if wrong:** the answer is a new type; the generated TypeScript changes with it.
5. **A typo locks every browser out; the admin socket is the way back.** (amended after the security review of 2026-10-02: A2, A4)
   - **The rule the brief suggested cannot be built:** "the new origin must equal the request's own `Origin`". `browser_rules` refuses every state-changing request whose `Origin` is not the current `public_url` (kernel §3.3), so that rule would mean "never change it".
   - **Choice (the review's (a)):** a wrong value is undone with `hennery admin reset-public-url` on the collector's machine (kernel §4.2), and the frontend warns before it saves. Its duties (frontend spec §8, Task 3): say that every device signs out, this one too; that passkeys go when the host name changes and OAuth registrations must be redone; that a wrong value locks every browser out until the admin command is run; show the new value as an origin and have it typed twice; send it only when it changed; after the 200, go to the new origin to sign in.
   - **The log:** every move over the API is logged at `warn` with the origin left, the origin taken and the counts, as soon as it is made (A2; `public_url_log.rs`). A stolen stepped-up session could move `public_url` to a host the attacker fronts, where the owner's next sign-in would happen; step-up is the guard, the log the trace. Origins are not secrets. The admin socket logs only the command's name (§4.2), unchanged.
   - **Alternatives:**
     - **Two-phase:** a pending `public_url`, accepted beside the old one until a request from the new origin confirms it. Two origins accepted at once weakens §3.3, and it is new state: a scope change.
     - **A server-side probe** of the new origin's `/healthz`, with a nonce proving it reaches this collector: wrong refusals under hairpin NAT, split DNS or Tailscale, and an outbound request to a URL the operator types.
   - **Cost if wrong:** a later plan adds the two-phase move beside this one.
6. **Subscriptions, streams and ceremonies end with the sessions, whatever races them.**
   - The same transaction deletes every subscription; the bump ends every stream (`a_change_ends_every_session_stream_subscription_and_ceremony`).
   - A subscribe racing the change: `Hosts::subscribe` re-checks its session in an `IMMEDIATE` transaction on its own connection, so it lands before (and is deleted) or finds its session gone (401).
   - A passkey finish racing it takes the operator's lock before it reads the relying party (3c A4): it finishes before, or reads the new one; a registration finds its ceremony cleared or its session gone.
7. **The cached-origin race (3b-ii D12) stays recorded, not closed.** (amended after the security review of 2026-10-02)
   - `browser_rules` reads the cached origin once per request. A request that passed it just before the change still runs its handler after it. The browser's own concurrent requests can now race the change, not only the admin socket.
   - Closed for this `PATCH` by A1. Re-checked in their own writes already: a passkey registration (3c A2), a push subscribe, a passkey step-up. Not re-checked: the other step-up actions (pairing codes, hosts, hats, path rules, sessions, gateway connections). Their effect is an action the stepped-up owner was allowed a moment earlier, finishing just after the move.
   - **A password login** that passes the `Origin` check before the change and opens its session after it outlives "every session ended"; on a port-only move its cookie reaches the new origin too. It proves the password, so it grants nothing new.
   - Closing it everywhere means an epoch on the cached origin, re-checked at each handler's commit. Recorded in "After this plan".
8. **What a URL accepted here may be.**
   - Setup's rule, parity with the admin socket: `https`, or `http` to loopback. From a remote browser a move to `http://localhost:…` is a guaranteed lockout that only the admin socket undoes; the frontend's warning covers it. Whether the API should refuse loopback `http` is recorded for the maintainer (the review's product question), with parity as the default.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **Wire types change in Task 2 only.** The generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated there with `cargo run -p hennery-proto --bin gen`; new root types go in both lists of `codegen.rs`. The web checks then run: `pnpm --dir web install --frozen-lockfile && pnpm --dir web typecheck && pnpm --dir web test`.
- **`Cargo.lock`** changes once, in Task 2's Step 1, by one `cargo build --workspace` without `--locked`; every other command runs `--locked`.
- Storage (kernel §1): every statement names the owner. The audit (`owner_filter.rs`) reads every statement of `operator.rs`; its floor is 29 after Task 1.
- Browser routes (kernel §3.3): state-changing methods need `public_url`'s `Origin` and a JSON body; the push routes read at most 16 KiB.
- No Linux-only code. The differential test drives the admin socket, a Unix socket, as the kernel's admin tests do; ubuntu ran it on the scratch CI PR (#97).
- Commits follow Conventional Commits and use the repository's own identity (gmail). Push the feature branch after every completed task; never push `main`.

## Review Focus

1. **The owner moves `public_url` from Settings, then signs in at the new origin.**
   - Expected: 200 with the new origin and the counts; one clearing cookie; every session, stream, subscription and ceremony ended; the old origin refused (403 `origin_mismatch`) for a signed-in `PATCH` and for a login; the new origin served; a restart reads the new value.
   - Tests: Task 2 `a_change_ends_every_session_stream_subscription_and_ceremony`.
2. **A stale session, or a body only the parser could tell apart.**
   - Expected: 403 `step_up_required` for any body naming `public_url`, an escaped key included, and nothing stored, a contact beside it neither; 422/400 for what is no key; no step-up for a contact alone or a `null` `public_url`.
   - Tests: Task 2 `naming_public_url_needs_a_fresh_step_up_and_a_contact_alone_does_not`.
3. **A session revoked, or a password reset, while the `PATCH` is in flight** (A1).
   - Expected: nothing changes; 401 with the cookie cleared; or 403 `step_up_required` with the cookie kept if the step-up lapsed.
   - Tests: Task 1 `a_change_whose_caller_ended_or_stepped_down_changes_nothing`; Task 2 `a_session_revoked_while_its_body_arrives_changes_nothing`, `a_step_up_lapsed_while_its_body_arrives_changes_nothing`.
4. **Parity with the admin socket.**
   - Expected: the same rows, cache, ceremonies, counts and announced ending, for a host change and a port change.
   - Tests: Task 2 `the_api_and_the_admin_socket_leave_the_same_state`.
5. **Passkeys on a host change and a port change, and the same origin again.**
   - Expected: removed for another host name; kept for another port; kept for the same origin, whose sessions still end.
   - Tests: Task 2 `a_host_change_removes_the_passkeys_and_a_port_change_keeps_them`, `the_same_origin_again_ends_every_session_and_keeps_the_passkeys`.
6. **The cookie between `https` and loopback `http`.**
   - Expected: cleared `Secure` from an `https` `public_url`, not from loopback `http`.
   - Tests: Task 2 `the_cookie_is_cleared_as_the_old_public_url_set_it`.
7. **Every refusal.**
   - Expected: 401, 403 `origin_mismatch`, 415, 413, 422, 400 for a URL and 400 for a contact, each changing nothing and setting no cookie; before setup 403 `setup_required`.
   - Tests: Task 2 `every_refusal_answers_its_code_and_changes_nothing`, `before_setup_the_browser_rules_answer_first`; Task 1 `a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-kernel/src/operator.rs` | `PublicUrlChange`, `change_public_url`, `reset_public_url` through it | 1 |
| `crates/hennery-kernel/tests/operator.rs`, `crates/hennery-testkit/tests/owner_filter.rs` | The transaction and the caller's re-check; the audit's floor | 1 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs`, the generated files | `SettingsUpdateRequest.public_url`, `SettingsUpdateResponse`, `PublicUrlChanged` | 2 |
| `crates/hennery-sessions/src/push.rs` | The conditional step-up, the change, the answer and the cookie, the log | 2 |
| `crates/hennery-testkit/Cargo.toml`, `Cargo.lock` | `tracing-subscriber` for the log test | 2 |
| `crates/hennery-testkit/tests/public_url.rs` (new), `public_url_log.rs` (new), `push.rs` | Over HTTP; the log; the old refusal goes | 2 |
| `docs/specs/2026-09-26-kernel-design.md`, `…-hennery-architecture-design.md`, `…-frontend-design.md` | The write-back | 3 |

All commands run from the repository root inside the dev shell (`nix develop -c …`). Work on a feature branch off `main`. Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate: `cargo build --workspace` rewrites `Cargo.lock`, and `cargo run -p hennery-proto --bin gen` the generated files. They change no other file.

---

### Task 1: One operator function for both paths

**Files:**
- Modify: `crates/hennery-kernel/src/operator.rs`
- Test: `crates/hennery-kernel/tests/operator.rs`; `crates/hennery-testkit/tests/owner_filter.rs` (`operator.rs`: 29 statements)

**Interfaces:**
- Produces: `pub enum PublicUrlChange { Done { from: Option<PublicUrl>, to: PublicUrl, sessions_ended: usize, passkeys_removed: usize }, NotSetUp, Invalid(String), SignedOut, StepUpRequired }`; `Operator::change_public_url(&self, input: &str, contact: Option<Option<&str>>, caller: Option<(&str, i64)>) -> Result<PublicUrlChange>`.
- Unchanged: `Operator::reset_public_url(&self, input: &str) -> Result<Reset>`, and so the admin socket.

- [ ] **Step 1: Write the failing tests**

The kernel's tests change `public_url` with and without a contact, with a refused contact and a refused URL (nothing changes), and with a caller whose session was revoked or whose step-up lapsed (nothing changes, nothing announced). The audit's floor rises by the two new statements.

In `crates/hennery-kernel/tests/operator.rs`, replace:

  ```rust
      MAX_PASSWORD_BYTES, Operator, PublicUrl, Reset, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE, SetupOutcome,
  ```

with:

  ```rust
      MAX_PASSWORD_BYTES, Operator, PublicUrl, PublicUrlChange, Reset, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE,
      STEP_UP_SECS, SetupOutcome,
  ```

In `crates/hennery-kernel/tests/operator.rs`, replace:

  ```rust

  /// The admin socket's `setup-url` (kernel spec §4.2): the link announced
  ```

with:

  ```rust

  /// Plan 4d-B4 decision 1: `PATCH /api/settings` changes `public_url` and
  /// the push contact through `change_public_url`, in one transaction. Both
  /// are checked before anything is written: a refused contact keeps the
  /// old URL and its sessions, a refused URL the old contact. `None` leaves
  /// the contact as it is, `Some(None)` clears it. `Done` names the origin
  /// left and the one taken (the review's A2).
  #[test]
  fn a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing() {
      let op = Operator::open_in_memory().unwrap();
      assert_eq!(
          op.change_public_url("https://moved.example", Some(Some("me@example.com")), None)
              .unwrap(),
          PublicUrlChange::NotSetUp
      );
      assert_eq!(op.contact().unwrap(), None);
      let token = op.issue_setup_token(NOW).unwrap().unwrap();
      let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
          panic!("setup failed");
      };
      op.set_contact(Some("old@example.com")).unwrap().unwrap();
      let session = op.open_session("browser", &phc, NOW).unwrap().unwrap();
      let unchanged = |op: &Operator| {
          assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
          assert_eq!(op.contact().unwrap().as_deref(), Some("old@example.com"));
          assert!(op.authenticate(&session, NOW).unwrap().is_some());
      };

      let refused = op
          .change_public_url("https://moved.example", Some(Some("me@example.com?cc=x")), None)
          .unwrap();
      assert!(matches!(refused, PublicUrlChange::Invalid(_)), "{refused:?}");
      unchanged(&op);
      let refused = op
          .change_public_url("http://moved.example", Some(Some("me@example.com")), None)
          .unwrap();
      assert!(matches!(refused, PublicUrlChange::Invalid(_)), "{refused:?}");
      unchanged(&op);

      assert_eq!(
          op.change_public_url("https://moved.example", Some(Some("me@example.com")), None)
              .unwrap(),
          PublicUrlChange::Done {
              from: Some(PublicUrl::parse("https://hennery.example").unwrap()),
              to: PublicUrl::parse("https://moved.example").unwrap(),
              sessions_ended: 1,
              passkeys_removed: 0
          }
      );
      assert_eq!(op.public_url().unwrap().origin(), "https://moved.example");
      assert_eq!(op.contact().unwrap().as_deref(), Some("me@example.com"));
      assert!(op.authenticate(&session, NOW).unwrap().is_none());

      op.change_public_url("https://hennery.example", None, None).unwrap();
      assert_eq!(op.contact().unwrap().as_deref(), Some("me@example.com"));
      op.change_public_url("https://moved.example", Some(None), None).unwrap();
      assert_eq!(op.contact().unwrap(), None);
  }

  /// The review's A1: the change re-checks its caller's session in its
  /// transaction, live and stepped up, as a passkey registration's write
  /// does. A session revoked, or a step-up lapsed, since the request's own
  /// checks changes nothing at all, and announces no ending.
  #[test]
  fn a_change_whose_caller_ended_or_stepped_down_changes_nothing() {
      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      let op = Operator::open(&db).unwrap();
      let token = op.issue_setup_token(NOW).unwrap().unwrap();
      let SetupOutcome::Done { phc, .. } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
          panic!("setup failed");
      };
      let id = |token: &str| op.authenticate(token, NOW).unwrap().unwrap().session_id;
      let revoked = id(&op.open_session("browser", &phc, NOW).unwrap().unwrap());
      let kept = op.open_session("browser", &phc, NOW).unwrap().unwrap();
      let kept_id = id(&kept);
      assert!(op.revoke_session(&revoked, NOW).unwrap());
      let dump = || {
          let conn = rusqlite::Connection::open(&db).unwrap();
          ["owners", "settings", "auth_sessions", "push_subscriptions", "passkeys"].map(|table| {
              let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
              let columns = stmt.column_count();
              stmt.query_map([], |r| {
                  Ok((0..columns)
                      .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
                      .collect::<Vec<_>>())
              })
              .unwrap()
              .collect::<rusqlite::Result<Vec<_>>>()
              .unwrap()
          })
      };
      let before = dump();
      let mut ends = op.session_ends();
      ends.borrow_and_update();

      for (caller, now, outcome) in [
          ((revoked.as_str(), NOW), NOW, PublicUrlChange::SignedOut),
          // Exactly five minutes after the last check: no longer fresh.
          (
              (kept_id.as_str(), NOW + STEP_UP_SECS),
              NOW + STEP_UP_SECS,
              PublicUrlChange::StepUpRequired,
          ),
      ] {
          assert_eq!(
              op.change_public_url("https://moved.example", Some(Some("me@example.com")), Some(caller))
                  .unwrap(),
              outcome
          );
          assert_eq!(dump(), before, "{outcome:?}");
          assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
          assert!(op.authenticate(&kept, now).unwrap().is_some());
      }
      assert!(!ends.has_changed().unwrap(), "an ending was announced");

      // A second inside the window, the change is made.
      assert!(matches!(
          op.change_public_url("https://moved.example", None, Some((&kept_id, NOW + STEP_UP_SECS - 1)))
              .unwrap(),
          PublicUrlChange::Done { sessions_ended: 1, .. }
      ));
  }

  /// The admin socket's `setup-url` (kernel spec §4.2): the link announced
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          27,
  ```

with:

  ```rust
          29,
  ```


- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c cargo test -p hennery-kernel --test operator --locked`
Expected: `operator` does not compile: `unresolved import hennery_kernel::operator::PublicUrlChange` and 8 × `no method named change_public_url found for struct Operator`.

- [ ] **Step 3: Write the implementation**

In `crates/hennery-kernel/src/operator.rs`, replace:

  ```rust
      /// changed.
      Invalid(String),
  }

  ```

with:

  ```rust
      /// changed.
      Invalid(String),
  }

  /// The outcome of `Operator::change_public_url`.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PublicUrlChange {
      /// Done, as `Reset::Done`; `from` and `to` were read and written under
      /// the lock that orders every change (plan 4d-B4's review, A2).
      Done {
          from: Option<PublicUrl>,
          to: PublicUrl,
          sessions_ended: usize,
          passkeys_removed: usize,
      },
      /// The owner is not set up yet.
      NotSetUp,
      /// The new `public_url` or contact is not acceptable (why); nothing
      /// changed.
      Invalid(String),
      /// The caller's session ended before the write; nothing changed.
      SignedOut,
      /// The caller's step-up lapsed before the write; nothing changed.
      StepUpRequired,
  }

  ```

In `crates/hennery-kernel/src/operator.rs`, replace:

  ```rust
          let public_url = match PublicUrl::parse(input) {
              Ok(url) => url,
              Err(problem) => return Ok(Reset::Invalid(problem)),
          };
          if !self.is_set_up()? {
              return Ok(Reset::NotSetUp);
          }
          let (ended, passkeys_removed) = {
              let mut conn = self.conn();
              let tx = conn.transaction()?;
  ```

with:

  ```rust
          Ok(match self.change_public_url(input, None, None)? {
              PublicUrlChange::Done {
                  sessions_ended,
                  passkeys_removed,
                  ..
              } => Reset::Done {
                  sessions_ended,
                  passkeys_removed,
              },
              PublicUrlChange::NotSetUp => Reset::NotSetUp,
              PublicUrlChange::Invalid(problem) => Reset::Invalid(problem),
              // Only a caller's session is re-checked, and the admin socket
              // names none.
              PublicUrlChange::SignedOut | PublicUrlChange::StepUpRequired => {
                  anyhow::bail!("the admin socket's reset re-checked a session")
              }
          })
      }

      /// What `reset_public_url` does, the one way `public_url` changes after
      /// setup: the admin socket's reset and `PATCH /api/settings` (plan 4d-B4
      /// decision 1) both call it.
      /// - With `contact`, the push contact is set too (`Some(None)` clears
      ///   it), in the same transaction: a body that names both changes both
      ///   or neither. Both are checked before anything is written; either
      ///   one refused is `Invalid`, nothing changed.
      /// - With `caller`, the session that asked and the time it asked: the
      ///   transaction's first statement re-checks that session, live and
      ///   stepped up, as a passkey registration's write does (plan 3c A2;
      ///   4d-B4's review, A1). One ended or stepped down since the request's
      ///   checks (a password reset, a revoke, a slow body) changes nothing.
      pub fn change_public_url(
          &self,
          input: &str,
          contact: Option<Option<&str>>,
          caller: Option<(&str, i64)>,
      ) -> Result<PublicUrlChange> {
          let public_url = match PublicUrl::parse(input) {
              Ok(url) => url,
              Err(problem) => return Ok(PublicUrlChange::Invalid(problem)),
          };
          if let Some(problem) = contact.flatten().and_then(crate::push::contact_problem) {
              return Ok(PublicUrlChange::Invalid(problem));
          }
          if !self.is_set_up()? {
              return Ok(PublicUrlChange::NotSetUp);
          }
          let (from, ended, passkeys_removed) = {
              let mut conn = self.conn();
              let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
              if let Some((session_id, now)) = caller {
                  let stepped_up_at: Option<Option<i64>> = tx
                      .query_row(
                          "SELECT last_step_up_at FROM auth_sessions
                           WHERE id_hash = ?1 AND owner_id = ?2 AND expires_at > ?3",
                          params![session_id, self.owner, now],
                          |r| r.get(0),
                      )
                      .optional()?;
                  match stepped_up_at {
                      None => return Ok(PublicUrlChange::SignedOut),
                      // `Authenticated::stepped_up`'s window.
                      Some(at) if !at.is_some_and(|at| now - at < STEP_UP_SECS) => {
                          return Ok(PublicUrlChange::StepUpRequired);
                      }
                      Some(_) => {}
                  }
              }
              if let Some(contact) = contact {
                  tx.execute(
                      "UPDATE owners SET contact = ?2 WHERE id = ?1",
                      params![self.owner, contact],
                  )?;
              }
  ```

In `crates/hennery-kernel/src/operator.rs`, replace:

  ```rust
              let same_host = self.public_url().as_ref().and_then(PublicUrl::rp_id) == public_url.rp_id();
  ```

with:

  ```rust
              // Read under the lock, which every writer of the cache holds.
              let from = self.public_url();
              let same_host = from.as_ref().and_then(PublicUrl::rp_id) == public_url.rp_id();
  ```

In `crates/hennery-kernel/src/operator.rs`, replace:

  ```rust
              *self.public_url.write().expect("public_url lock") = Some(public_url);
              self.ceremonies.clear();
              (ended, passkeys_removed)
          };
          self.sessions_ended();
          Ok(Reset::Done {
  ```

with:

  ```rust
              *self.public_url.write().expect("public_url lock") = Some(public_url.clone());
              self.ceremonies.clear();
              (from, ended, passkeys_removed)
          };
          self.sessions_ended();
          Ok(PublicUrlChange::Done {
              from,
              to: public_url,
  ```


- [ ] **Step 4: Run the tests, and see them pass**

Run: `nix develop -c cargo test -p hennery-kernel --test operator --test admin --test passkeys --test push --locked && nix develop -c cargo test -p hennery-testkit --test owner_filter --test step_up --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probes**

Each on this task's tree; each must make a test fail, then be restored.
In `crates/hennery-kernel/src/operator.rs`, `change_public_url`:
- P1: In `change_public_url`, replace `if let Some(contact) = contact {` with `if let Some(contact) = contact.filter(|_| false) {` (the contact never written). Fails: `a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing`. Restore it.
- P2: Store the row under the key `"probe"` instead of `PUBLIC_URL_KEY` (the value cached, not stored). Fails: `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`, `a_change_ends_every_session_stream_subscription_and_ceremony`. Restore it.
- P3: Add `AND 0` to its `DELETE FROM auth_sessions`. Fails: `a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing`, `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`, `a_change_whose_caller_ended_or_stepped_down_changes_nothing`, `a_host_change_removes_the_passkeys_and_a_port_change_keeps_them`, `the_same_origin_again_ends_every_session_and_keeps_the_passkeys`, `a_change_ends_every_session_stream_subscription_and_ceremony`, `a_change_over_the_api_is_logged_with_both_origins`. Restore it.
- P4: Add `AND 0` to its `DELETE FROM push_subscriptions`. Fails: `a_change_ends_every_session_stream_subscription_and_ceremony`. Restore it.
- P5a: Replace `if same_host {` with `if true {`. Fails: `a_change_ends_every_session_stream_subscription_and_ceremony`, `a_host_change_removes_the_passkeys_and_a_port_change_keeps_them`. Restore it.
- P5b: Replace `if same_host {` with `if false {` (remove on every move). Fails: `a_host_change_removes_the_passkeys_and_a_port_change_keeps_them`, `the_same_origin_again_ends_every_session_and_keeps_the_passkeys`. Restore it.
- P6: Replace the cache's write with `let _ = public_url.clone();`. Fails: `a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing`, `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`, `the_cookie_is_cleared_as_the_old_public_url_set_it`, `a_host_change_removes_the_passkeys_and_a_port_change_keeps_them`, `a_change_ends_every_session_stream_subscription_and_ceremony`. Restore it.
- P7: Remove its `self.ceremonies.clear();`. Fails: `a_change_ends_every_session_stream_subscription_and_ceremony`, `the_api_and_the_admin_socket_leave_the_same_state`. Restore it.
- P8: Remove its `self.sessions_ended();`. Fails: `a_change_ends_every_session_stream_subscription_and_ceremony`, `the_api_and_the_admin_socket_leave_the_same_state`. Restore it.
- P9: Replace `None => return Ok(PublicUrlChange::SignedOut),` with `None => {}`. Fails: `a_change_whose_caller_ended_or_stepped_down_changes_nothing`, `a_session_revoked_while_its_body_arrives_changes_nothing`. Restore it.
- P10: Replace the `StepUpRequired` arm's guard with `if false` (`Some(_at) if false =>`). Fails: `a_change_whose_caller_ended_or_stepped_down_changes_nothing`, `a_step_up_lapsed_while_its_body_arrives_changes_nothing`. Restore it.
- P20: Remove the `if !self.is_set_up()?` check (`NotSetUp`). Fails: `a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing`, `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`. Restore it.
- P21: Never refuse the contact (`.and_then(crate::push::contact_problem).filter(|_| false)`). Fails: `a_public_url_change_sets_the_contact_in_its_transaction_or_changes_nothing`, `every_refusal_answers_its_code_and_changes_nothing`. Restore it.

In `reset_public_url`, the mapping back to `Reset`:
- P24: Map `NotSetUp` to `Reset::Invalid(String::new())`. Fails: `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`. Restore it.
- P25: Map `Invalid` to `Reset::NotSetUp`. Fails: `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`. Restore it.
- P26: Swap `sessions_ended` and `passkeys_removed` in `Reset::Done`. Fails: `a_public_url_reset_replaces_the_cached_origin_and_ends_every_session`, `the_api_and_the_admin_socket_leave_the_same_state`. Restore it.

(Run on the whole plan's tree, so each list names the tests of both tasks that fail.)

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **1401 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-kernel crates/hennery-testkit/tests/owner_filter.rs
git commit -m "feat(kernel): one function changes public_url for the admin socket and the API"
```

### Task 2: `PATCH /api/settings {public_url}`

**Files:**
- Modify: `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, `crates/hennery-sessions/src/push.rs`, `crates/hennery-testkit/Cargo.toml`
- Regenerate: `Cargo.lock`, `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-testkit/tests/public_url.rs` (new), `crates/hennery-testkit/tests/public_url_log.rs` (new), `crates/hennery-testkit/tests/push.rs`

**Interfaces:**
- Consumes: Task 1's `change_public_url` and `PublicUrlChange`.
- Produces: `SettingsUpdateRequest { contact?, public_url? }`; `SettingsUpdateResponse { public_url, contact?, public_url_changed? }`; `PublicUrlChanged { sessions_ended, passkeys_removed }`. `GET /api/settings` still answers `SettingsResponse`.

- [ ] **Step 1: Write the failing tests**

Over HTTP, against a collector on one database file: the conditional step-up and the bodies only a parser could tell apart; a change and everything it ends; the cookie both ways; passkeys on a host and a port change and the same origin again; every refusal; before setup; a session revoked, and one whose step-up lapses, while its body arrives; and the API against the admin socket on a copy of one database. The log line has a binary of its own. `push.rs` stops expecting a `public_url` to be refused.

In `crates/hennery-testkit/Cargo.toml`, replace:

  ```toml
  tokio-tungstenite.workspace = true
  webauthn-authenticator-rs.workspace = true
  ```

with:

  ```toml
  tokio-tungstenite.workspace = true
  # `public_url_log.rs` reads what the collector logs (plan 4d-B4, A2).
  tracing-subscriber.workspace = true
  webauthn-authenticator-rs.workspace = true
  ```

Create `crates/hennery-testkit/tests/public_url.rs`:

  ```rust
  //! `PATCH /api/settings {public_url}` (kernel spec §3.2, §3.4, §8; plan
  //! 4d-B4): the API moves the collector exactly as `hennery admin
  //! reset-public-url` does, through the same operator function, behind a
  //! fresh step-up that only a body naming `public_url` needs.

  use hennery_kernel::admin::{ADMIN_SOCKET, Admin, AdminRequest, AdminResponse};
  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::Operator;
  use hennery_kernel::passkeys::Start;
  use hennery_kernel::secret::{sha256_hex, unix_now};
  use hennery_proto::rest::{ApiError, PublicUrlChanged, SettingsUpdateResponse};
  use hennery_sessions::AppState;
  use hennery_sessions::store::Store;
  use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
  use std::net::SocketAddr;
  use std::path::{Path, PathBuf};
  use std::time::Duration;
  use webauthn_authenticator_rs::WebauthnAuthenticator;
  use webauthn_authenticator_rs::softpasskey::SoftPasskey;
  use webauthn_rs::prelude::{CreationChallengeResponse, Url};

  /// A browser's subscription (web-push-native's example keys).
  const SUBSCRIBE: &str = r#"{"endpoint":"https://fcm.googleapis.com/fcm/send/x","keys":{"p256dh":"BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY","auth":"_ordMnz7uTCmrpBTeUV4Bw"}}"#;

  /// The kernel's tables a `public_url` change reads or writes.
  const TABLES: &[&str] = &[
      "owners",
      "settings",
      "password_credentials",
      "auth_sessions",
      "push_subscriptions",
      "passkeys",
  ];

  struct Collector {
      addr: SocketAddr,
      state: AppState,
      db: PathBuf,
  }

  impl Collector {
      /// A collector on `db`, set up at `PUBLIC_URL` with no session signed
      /// in, unless it was set up already (a copy).
      async fn on(db: &Path) -> Self {
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let state = AppState::new(
              Store::open(db).unwrap(),
              Hosts::open(db).unwrap(),
              Operator::open(db).unwrap(),
          );
          if !state.operator.is_set_up().unwrap() {
              let token = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
              state
                  .operator
                  .set_up(&token, OWNER_PASSWORD, PUBLIC_URL, unix_now())
                  .unwrap();
              // The session `stream` follows: a stream is 404 for an unknown one.
              state
                  .store
                  .create_session("s-1", "host-9", "fake", "/tmp", "hat-9", None)
                  .unwrap();
          }
          tokio::spawn(hennery_sessions::serve(listener, state.clone()));
          Self {
              addr,
              state,
              db: db.to_path_buf(),
          }
      }

      /// A session whose last password check was `age` seconds ago, last
      /// seen then too.
      fn session(&self, age: i64) -> String {
          let phc = hennery_testkit::owner_phc(&self.state.operator);
          self.state
              .operator
              .open_session("test", &phc, unix_now() - age)
              .unwrap()
              .unwrap()
      }

      fn request(&self, session: &str, origin: &str, method: &str, path: &str) -> reqwest::RequestBuilder {
          reqwest::Client::new()
              .request(method.parse().unwrap(), format!("http://{}{path}", self.addr))
              .header("origin", origin)
              .header("cookie", format!("hennery_session={session}"))
      }

      /// `PATCH /api/settings` with `body` as it is, from `origin`.
      async fn patch_from(&self, session: &str, origin: &str, body: &str) -> reqwest::Response {
          self.request(session, origin, "PATCH", "/api/settings")
              .header("content-type", "application/json")
              .body(body.to_string())
              .send()
              .await
              .unwrap()
      }

      async fn patch(&self, session: &str, body: &str) -> reqwest::Response {
          self.patch_from(session, PUBLIC_URL, body).await
      }

      /// Every row of the kernel's `TABLES`, as text.
      fn dump(&self) -> Vec<(String, Vec<String>)> {
          dump(&self.db)
      }

      /// Register a passkey at `PUBLIC_URL` from `session` (stepped up).
      fn register(&self, session: &str, label: &str) {
          let op = &self.state.operator;
          let id = sha256_hex(session.as_bytes());
          let Start::Begun { ceremony_id, options } = op.start_passkey_registration(&id, label, unix_now()).unwrap()
          else {
              panic!("the registration did not begin");
          };
          let options: CreationChallengeResponse = serde_json::from_value(options).unwrap();
          let credential = WebauthnAuthenticator::new(SoftPasskey::new(true))
              .do_registration(Url::parse(PUBLIC_URL).unwrap(), options)
              .unwrap();
          op.finish_passkey_registration(
              &id,
              &ceremony_id,
              &serde_json::to_value(credential).unwrap(),
              unix_now(),
          )
          .unwrap()
          .expect("registered");
      }

      /// Subscribe a browser to push from `session` (stepped up).
      async fn subscribe(&self, session: &str, endpoint: &str) {
          let body = SUBSCRIBE.replace("https://fcm.googleapis.com/fcm/send/x", endpoint);
          let resp = self
              .request(session, PUBLIC_URL, "POST", "/api/push/subscriptions")
              .header("content-type", "application/json")
              .body(body)
              .send()
              .await
              .unwrap();
          assert_eq!(resp.status(), 201);
      }

      /// Open the SSE stream of `s-1` with `session`.
      async fn stream(&self, session: &str) -> reqwest::Response {
          let resp = self
              .request(session, PUBLIC_URL, "GET", "/api/stream/sessions/s-1")
              .send()
              .await
              .unwrap();
          assert_eq!(resp.status(), 200);
          resp
      }

      async fn login_from(&self, origin: &str) -> u16 {
          reqwest::Client::new()
              .post(format!("http://{}/api/auth/login", self.addr))
              .header("origin", origin)
              .json(&serde_json::json!({ "password": OWNER_PASSWORD }))
              .send()
              .await
              .unwrap()
              .status()
              .as_u16()
      }
  }

  fn dump(db: &Path) -> Vec<(String, Vec<String>)> {
      let conn = rusqlite::Connection::open(db).unwrap();
      TABLES
          .iter()
          .map(|table| {
              let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
              let columns = stmt.column_count();
              let rows = stmt
                  .query_map([], |r| {
                      let values: Vec<String> = (0..columns)
                          .map(|i| format!("{:?}", r.get::<_, rusqlite::types::Value>(i).unwrap()))
                          .collect();
                      Ok(values.join("|"))
                  })
                  .unwrap()
                  .collect::<rusqlite::Result<Vec<_>>>()
                  .unwrap();
              (table.to_string(), rows)
          })
          .collect()
  }

  async fn code_of(resp: reqwest::Response) -> (u16, String) {
      let status = resp.status().as_u16();
      (
          status,
          resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
      )
  }

  /// Whether the SSE body of `stream` ends within `within`.
  async fn ends(stream: reqwest::Response, within: Duration) -> bool {
      use futures::StreamExt;
      let mut body = stream.bytes_stream();
      tokio::time::timeout(within, async { while let Some(Ok(_)) = body.next().await {} })
          .await
          .is_ok()
  }

  fn set_cookies(resp: &reqwest::Response) -> Vec<String> {
      resp.headers()
          .get_all("set-cookie")
          .iter()
          .map(|v| v.to_str().unwrap().to_string())
          .collect()
  }

  /// Decision 2 (kernel spec §3.4, §6): a body naming `public_url` needs a
  /// fresh step-up, checked before anything is read or written, so a stale
  /// session's `contact` beside it is not stored either. A body without one
  /// needs none, and neither does a `null` one, which changes nothing. The
  /// same bytes decoded another way never give another verdict: an escaped
  /// key is the key, and a duplicate, another case or a stray space is no
  /// key at all.
  #[tokio::test]
  async fn naming_public_url_needs_a_fresh_step_up_and_a_contact_alone_does_not() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      let stale = c.session(5 * 60);
      // Slid now, so no request below changes the session's row.
      let resp = c
          .request(&stale, PUBLIC_URL, "GET", "/api/settings")
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 200);
      let before = c.dump();
      let escaped = format!(r#"{{"public{}u005furl":"https://moved.example"}}"#, '\\');
      assert_eq!(escaped.as_bytes()[8], b'\\', "{escaped}");
      for (body, expected) in [
          (r#"{"public_url":"https://moved.example"}"#, (403, "step_up_required")),
          (
              r#"{"contact":"me@example.com","public_url":"https://moved.example"}"#,
              (403, "step_up_required"),
          ),
          (r#"{"public_url":"not a url"}"#, (403, "step_up_required")),
          (r#"{"public_url":""}"#, (403, "step_up_required")),
          // `public\u005furl`, built so no tool on the way can decode it.
          (escaped.as_str(), (403, "step_up_required")),
          (
              r#"{"public_url":null,"public_url":"https://moved.example"}"#,
              (422, "invalid_body"),
          ),
          (
              r#"{"public_url":"https://moved.example","public_url":"https://other.example"}"#,
              (422, "invalid_body"),
          ),
          (r#"{"Public_url":"https://moved.example"}"#, (422, "invalid_body")),
          (r#"{"public_url ":"https://moved.example"}"#, (422, "invalid_body")),
          (r#"{"public_url":7117}"#, (422, "invalid_body")),
          (
              "\u{feff}{\"public_url\":\"https://moved.example\"}",
              (400, "invalid_body"),
          ),
      ] {
          let resp = c.patch(&stale, body).await;
          assert_eq!(code_of(resp).await, (expected.0, expected.1.into()), "{body}");
          assert_eq!(c.dump(), before, "{body}");
      }
      assert!(c.state.operator.authenticate(&stale, unix_now()).unwrap().is_some());

      let resp = c.patch(&stale, r#"{"public_url":null}"#).await;
      assert_eq!(resp.status(), 200);
      assert!(set_cookies(&resp).is_empty());
      assert_eq!(c.dump(), before);
      let resp = c
          .patch(&stale, r#"{"public_url":null,"contact":"me@example.com"}"#)
          .await;
      assert_eq!(resp.status(), 200);
      assert!(set_cookies(&resp).is_empty());
      assert_eq!(c.state.operator.contact().unwrap().as_deref(), Some("me@example.com"));
      let resp = c.patch(&stale, r#"{"contact":"you@example.com"}"#).await;
      assert_eq!(resp.status(), 200);
      assert!(set_cookies(&resp).is_empty());
      let settings: SettingsUpdateResponse = resp.json().await.unwrap();
      assert_eq!(
          settings,
          SettingsUpdateResponse {
              public_url: PUBLIC_URL.into(),
              contact: Some("you@example.com".into()),
              public_url_changed: None,
          }
      );
      assert!(c.state.operator.authenticate(&stale, unix_now()).unwrap().is_some());
  }

  /// Decisions 1, 3 and 4: a change ends every signed-in session, the
  /// caller's with the others, and so their streams and push subscriptions,
  /// and every passkey ceremony under way. The answer reports it as the
  /// admin socket does, and clears the cookie once, on a request that slid
  /// its session too. The old origin is refused from then on; the new one
  /// signs in.
  #[tokio::test]
  async fn a_change_ends_every_session_stream_subscription_and_ceremony() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      // Last seen two minutes ago, so this request slides it.
      let mine = c.session(120);
      let other = c.session(0);
      c.subscribe(&mine, "https://fcm.googleapis.com/fcm/send/mine").await;
      c.subscribe(&other, "https://fcm.googleapis.com/fcm/send/other").await;
      let stream = c.stream(&other).await;
      assert!(matches!(
          c.state.operator.start_passkey_login(unix_now()).unwrap(),
          Start::NoPasskeys
      ));
      c.register(&other, "laptop");
      assert!(matches!(
          c.state.operator.start_passkey_login(unix_now()).unwrap(),
          Start::Begun { .. }
      ));
      assert!(!c.state.operator.ceremonies.is_empty());
      let mut ends_seen = c.state.operator.session_ends();
      ends_seen.borrow_and_update();

      let resp = c.patch(&mine, r#"{"public_url":"https://Moved.Example/"}"#).await;
      assert_eq!(resp.status(), 200);
      let cookies = set_cookies(&resp);
      assert_eq!(cookies.len(), 1, "{cookies:?}");
      assert_eq!(
          cookies[0],
          "hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure"
      );
      let settings: SettingsUpdateResponse = resp.json().await.unwrap();
      assert_eq!(
          settings,
          SettingsUpdateResponse {
              public_url: "https://moved.example".into(),
              contact: None,
              public_url_changed: Some(PublicUrlChanged {
                  sessions_ended: 2,
                  passkeys_removed: 1,
              }),
          }
      );
      assert!(ends_seen.has_changed().unwrap(), "the ending was not announced");
      assert!(
          ends(stream, Duration::from_secs(1)).await,
          "a stream outlived the change"
      );
      for session in [&mine, &other] {
          assert!(c.state.operator.authenticate(session, unix_now()).unwrap().is_none());
      }
      assert!(c.state.hosts.subscriptions().unwrap().is_empty());
      assert!(c.state.operator.passkeys().unwrap().is_empty());
      assert!(c.state.operator.ceremonies.is_empty());
      // Stored, not only cached: a restart keeps it.
      assert_eq!(
          Operator::open(&c.db).unwrap().public_url().unwrap().origin(),
          "https://moved.example"
      );

      // The old origin is refused, signed in or not; the new one is served.
      let fresh = c.session(0);
      let resp = c.patch(&fresh, r#"{"contact":"me@example.com"}"#).await;
      assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()));
      assert_eq!(c.login_from(PUBLIC_URL).await, 403);
      let resp = c
          .patch_from(&fresh, "https://moved.example", r#"{"contact":"me@example.com"}"#)
          .await;
      assert_eq!(resp.status(), 200);
      assert_eq!(c.login_from("https://moved.example").await, 204);
  }

  /// Decision 4: the cookie is cleared as the old `public_url` set it, since
  /// the answer goes to the old origin: `Secure` from an `https` one, though
  /// the new one is loopback `http`, and not from loopback `http`.
  #[tokio::test]
  async fn the_cookie_is_cleared_as_the_old_public_url_set_it() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      let resp = c
          .patch(&c.session(0), r#"{"public_url":"http://localhost:7117"}"#)
          .await;
      assert_eq!(resp.status(), 200);
      assert_eq!(
          set_cookies(&resp),
          ["hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure"]
      );
      let resp = c
          .patch_from(
              &c.session(0),
              "http://localhost:7117",
              r#"{"public_url":"https://hennery.example"}"#,
          )
          .await;
      assert_eq!(resp.status(), 200);
      assert_eq!(
          set_cookies(&resp),
          ["hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"]
      );
  }

  /// Plan 3c decision 9, now over the API: a change of host name removes
  /// every passkey; one of port keeps them.
  #[tokio::test]
  async fn a_host_change_removes_the_passkeys_and_a_port_change_keeps_them() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      let session = c.session(0);
      c.register(&session, "laptop");
      c.register(&session, "phone");
      let changed = |resp: SettingsUpdateResponse| resp.public_url_changed.unwrap();
      let resp = c
          .patch(&session, r#"{"public_url":"https://hennery.example:8443"}"#)
          .await;
      assert_eq!(
          changed(resp.json().await.unwrap()),
          PublicUrlChanged {
              sessions_ended: 1,
              passkeys_removed: 0
          }
      );
      assert_eq!(c.state.operator.passkeys().unwrap().len(), 2);
      let resp = c
          .patch_from(
              &c.session(0),
              "https://hennery.example:8443",
              r#"{"public_url":"https://moved.example"}"#,
          )
          .await;
      assert_eq!(
          changed(resp.json().await.unwrap()),
          PublicUrlChanged {
              sessions_ended: 1,
              passkeys_removed: 2
          }
      );
      assert!(c.state.operator.passkeys().unwrap().is_empty());
  }

  /// Decision 3: the same origin again is a change like any other, as the
  /// admin socket's reset is: every session ends, the passkeys stay (the
  /// host name is the same).
  #[tokio::test]
  async fn the_same_origin_again_ends_every_session_and_keeps_the_passkeys() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      let session = c.session(0);
      let other = c.session(0);
      c.register(&session, "laptop");
      let resp = c.patch(&session, &format!(r#"{{"public_url":"{PUBLIC_URL}"}}"#)).await;
      assert_eq!(resp.status(), 200);
      assert_eq!(set_cookies(&resp).len(), 1);
      let settings: SettingsUpdateResponse = resp.json().await.unwrap();
      assert_eq!(settings.public_url, PUBLIC_URL);
      assert_eq!(
          settings.public_url_changed,
          Some(PublicUrlChanged {
              sessions_ended: 2,
              passkeys_removed: 0
          })
      );
      assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_none());
      assert_eq!(c.state.operator.passkeys().unwrap().len(), 1);
  }

  /// Every refusal of a body naming `public_url`, each with its code, and
  /// none changes anything: no session, no cookie, a URL refused keeps the
  /// contact beside it and a contact refused the URL (decision 1).
  #[tokio::test]
  async fn every_refusal_answers_its_code_and_changes_nothing() {
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      c.state.operator.set_contact(Some("old@example.com")).unwrap().unwrap();
      let session = c.session(0);
      let before = c.dump();
      let move_body = r#"{"public_url":"https://moved.example"}"#;
      let refusals = [
          // No cookie.
          (
              reqwest::Client::new()
                  .patch(format!("http://{}/api/settings", c.addr))
                  .header("origin", PUBLIC_URL)
                  .json(&serde_json::json!({ "public_url": "https://moved.example" }))
                  .send()
                  .await
                  .unwrap(),
              (401, "unauthenticated"),
          ),
          (
              c.patch_from(&session, "https://moved.example", move_body).await,
              (403, "origin_mismatch"),
          ),
          (
              c.request(&session, PUBLIC_URL, "PATCH", "/api/settings")
                  .header("content-type", "text/plain")
                  .body(move_body)
                  .send()
                  .await
                  .unwrap(),
              (415, "unsupported_media_type"),
          ),
          (
              c.patch(
                  &session,
                  &format!(
                      r#"{{"public_url":"https://moved.example","contact":"{}"}}"#,
                      "a".repeat(hennery_kernel::auth_api::MAX_BODY_BYTES)
                  ),
              )
              .await,
              (413, "body_too_large"),
          ),
          (
              c.patch(&session, r#"{"public_url":"https://moved.example","name":"x"}"#)
                  .await,
              (422, "invalid_body"),
          ),
          (
              c.patch(&session, r#"{"public_url":"http://moved.example"}"#).await,
              (400, "invalid"),
          ),
          (
              c.patch(
                  &session,
                  r#"{"public_url":"https://moved.example/app","contact":"me@example.com"}"#,
              )
              .await,
              (400, "invalid"),
          ),
          (
              c.patch(
                  &session,
                  r#"{"public_url":"https://moved.example","contact":"me@example.com?cc=x"}"#,
              )
              .await,
              (400, "invalid"),
          ),
      ];
      for (resp, (status, code)) in refusals {
          assert!(set_cookies(&resp).is_empty(), "{code}");
          assert_eq!(code_of(resp).await, (status, code.into()));
          assert_eq!(c.dump(), before, "{code}");
      }
      assert_eq!(c.state.operator.public_url().unwrap().origin(), PUBLIC_URL);
      assert!(c.state.operator.authenticate(&session, unix_now()).unwrap().is_some());
  }

  /// The review's A1, over HTTP: a session revoked while its request's body
  /// is still arriving, so after `require_operator` let it through (its
  /// slide shows it ran), changes nothing. The answer is 401 and clears the
  /// cookie (O2).
  #[tokio::test]
  async fn a_session_revoked_while_its_body_arrives_changes_nothing() {
      use tokio::io::{AsyncReadExt, AsyncWriteExt};
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      // Seen two minutes ago, so authenticating it slides its expiry.
      let token = c.session(120);
      let id = sha256_hex(token.as_bytes());
      let expiry = c.state.operator.session_expires_at(&id, unix_now()).unwrap().unwrap();
      let body = r#"{"public_url":"https://moved.example"}"#;
      let mut stream = tokio::net::TcpStream::connect(c.addr).await.unwrap();
      let head = format!(
          "PATCH /api/settings HTTP/1.1\r\nhost: {}\r\norigin: {PUBLIC_URL}\r\ncookie: hennery_session={token}\r\n\
           content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
          c.addr,
          body.len()
      );
      stream.write_all(head.as_bytes()).await.unwrap();
      stream.write_all(&body.as_bytes()[..10]).await.unwrap();
      let deadline = std::time::Instant::now() + Duration::from_secs(10);
      while c.state.operator.session_expires_at(&id, unix_now()).unwrap() == Some(expiry) {
          assert!(std::time::Instant::now() < deadline, "the session never slid");
          tokio::time::sleep(Duration::from_millis(10)).await;
      }
      assert!(c.state.operator.revoke_session(&id, unix_now()).unwrap());
      let before = c.dump();
      stream.write_all(&body.as_bytes()[10..]).await.unwrap();
      let mut answer = String::new();
      stream.read_to_string(&mut answer).await.unwrap();
      let answer = answer.to_ascii_lowercase();
      assert!(answer.starts_with("http/1.1 401"), "{answer}");
      assert!(answer.contains(r#""code":"unauthenticated""#), "{answer}");
      assert!(
          answer.contains("set-cookie: hennery_session=; httponly; samesite=strict; path=/; max-age=0; secure\r\n"),
          "{answer}"
      );
      assert_eq!(c.dump(), before);
      assert_eq!(c.state.operator.public_url().unwrap().origin(), PUBLIC_URL);
  }

  /// The review's A1, the other way: a session whose step-up lapses while
  /// its request's body is still arriving, after the handler's own check let
  /// it through, changes nothing, the contact beside the URL included. The
  /// answer is 403 `step_up_required`; the session lives on, so its cookie
  /// is not cleared.
  #[tokio::test]
  async fn a_step_up_lapsed_while_its_body_arrives_changes_nothing() {
      use tokio::io::{AsyncReadExt, AsyncWriteExt};
      let dir = tempfile::tempdir().unwrap();
      let c = Collector::on(&dir.path().join("hennery.db")).await;
      c.state.operator.set_contact(Some("old@example.com")).unwrap().unwrap();
      // Checked two minutes ago: fresh, and authenticating it slides its
      // expiry.
      let token = c.session(120);
      let id = sha256_hex(token.as_bytes());
      let expiry = c.state.operator.session_expires_at(&id, unix_now()).unwrap().unwrap();
      let body = r#"{"public_url":"https://moved.example","contact":"me@example.com"}"#;
      let mut stream = tokio::net::TcpStream::connect(c.addr).await.unwrap();
      let head = format!(
          "PATCH /api/settings HTTP/1.1\r\nhost: {}\r\norigin: {PUBLIC_URL}\r\ncookie: hennery_session={token}\r\n\
           content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
          c.addr,
          body.len()
      );
      stream.write_all(head.as_bytes()).await.unwrap();
      stream.write_all(&body.as_bytes()[..10]).await.unwrap();
      let deadline = std::time::Instant::now() + Duration::from_secs(10);
      while c.state.operator.session_expires_at(&id, unix_now()).unwrap() == Some(expiry) {
          assert!(std::time::Instant::now() < deadline, "the session never slid");
          tokio::time::sleep(Duration::from_millis(10)).await;
      }
      // The last check is now exactly five minutes old: no longer fresh.
      let stepped_down = rusqlite::Connection::open(&c.db)
          .unwrap()
          .execute(
              "UPDATE auth_sessions SET last_step_up_at = ?1 WHERE id_hash = ?2",
              rusqlite::params![unix_now() - hennery_kernel::operator::STEP_UP_SECS, id],
          )
          .unwrap();
      assert_eq!(stepped_down, 1);
      let before = c.dump();
      stream.write_all(&body.as_bytes()[10..]).await.unwrap();
      let mut answer = String::new();
      stream.read_to_string(&mut answer).await.unwrap();
      let answer = answer.to_ascii_lowercase();
      assert!(answer.starts_with("http/1.1 403"), "{answer}");
      assert!(answer.contains(r#""code":"step_up_required""#), "{answer}");
      // The slide's cookie, renewed; never a cleared one.
      assert!(!answer.contains("hennery_session=;"), "{answer}");
      assert!(!answer.contains("max-age=0"), "{answer}");
      assert_eq!(c.dump(), before);
      assert_eq!(c.state.operator.public_url().unwrap().origin(), PUBLIC_URL);
      assert_eq!(c.state.operator.contact().unwrap().as_deref(), Some("old@example.com"));
      assert!(c.state.operator.authenticate(&token, unix_now()).unwrap().is_some());
  }

  /// `Reset::NotSetUp` cannot be answered over HTTP: before setup the
  /// browser rules refuse every state-changing request first.
  #[tokio::test]
  async fn before_setup_the_browser_rules_answer_first() {
      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      let state = AppState::new(
          Store::open(&db).unwrap(),
          Hosts::open(&db).unwrap(),
          Operator::open(&db).unwrap(),
      );
      tokio::spawn(hennery_sessions::serve(listener, state.clone()));
      let resp = reqwest::Client::new()
          .patch(format!("http://{addr}/api/settings"))
          .header("origin", PUBLIC_URL)
          .json(&serde_json::json!({ "public_url": PUBLIC_URL }))
          .send()
          .await
          .unwrap();
      assert_eq!(code_of(resp).await, (403, "setup_required".into()));
      assert_eq!(state.operator.public_url(), None);
  }

  /// Decision 1's parity: the API and the admin socket, given the same
  /// database, leave it in the same state (the owner and its contact, the
  /// settings, the password, sessions, subscriptions and passkeys), the
  /// same cached origin and no ceremony, and both announce the ending. Once
  /// for a host change, once for a port change, so both of decision 9's
  /// branches are compared.
  #[tokio::test]
  async fn the_api_and_the_admin_socket_leave_the_same_state() {
      for target in ["https://moved.example", "https://hennery.example:8443"] {
          let dir = tempfile::tempdir().unwrap();
          let (api_dir, admin_dir) = (dir.path().join("api"), dir.path().join("admin"));
          std::fs::create_dir_all(&api_dir).unwrap();
          std::fs::create_dir_all(&admin_dir).unwrap();
          // Seeded once: a contact, two sessions, a subscription each, two
          // passkeys. Then copied whole, the WAL included.
          let api = Collector::on(&api_dir.join("hennery.db")).await;
          api.state.operator.set_contact(Some("me@example.com")).unwrap().unwrap();
          let caller = api.session(0);
          let other = api.session(0);
          api.subscribe(&caller, "https://fcm.googleapis.com/fcm/send/caller")
              .await;
          api.subscribe(&other, "https://fcm.googleapis.com/fcm/send/other").await;
          api.register(&caller, "laptop");
          api.register(&other, "phone");
          let copy = admin_dir.join("hennery.db");
          rusqlite::Connection::open(&api.db)
              .unwrap()
              .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
              .unwrap();
          let admin = Collector::on(&copy).await;
          assert_eq!(admin.dump(), api.dump(), "the copy differs");

          let mut announced = Vec::new();
          for c in [&api, &admin] {
              assert!(matches!(
                  c.state.operator.start_passkey_login(unix_now()).unwrap(),
                  Start::Begun { .. }
              ));
              let mut ends = c.state.operator.session_ends();
              ends.borrow_and_update();
              announced.push(ends);
          }

          let resp = api.patch(&caller, &format!(r#"{{"public_url":"{target}"}}"#)).await;
          assert_eq!(resp.status(), 200);
          let changed = resp
              .json::<SettingsUpdateResponse>()
              .await
              .unwrap()
              .public_url_changed
              .unwrap();

          let socket = admin_dir.join(ADMIN_SOCKET);
          let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
          tokio::spawn(hennery_kernel::admin::serve(
              hennery_kernel::admin::bind(&admin_dir).unwrap().expect("a short path"),
              Admin {
                  operator: admin.state.operator.clone(),
                  hosts: admin.state.hosts.clone(),
                  dir: admin_dir.clone(),
                  base_url: PUBLIC_URL.into(),
              },
              async move {
                  let _ = stopped.await;
              },
          ));
          let answer = hennery_kernel::admin::request(
              &socket,
              &AdminRequest::ResetPublicUrl {
                  public_url: target.into(),
              },
          )
          .await
          .unwrap();
          drop(stop);
          let AdminResponse::PublicUrlReset {
              public_url,
              sessions_ended,
              passkeys_removed,
          } = answer
          else {
              panic!("{answer:?}");
          };
          assert_eq!(
              (sessions_ended as u64, passkeys_removed as u64),
              (changed.sessions_ended, changed.passkeys_removed),
              "{target}"
          );
          assert_eq!(public_url, api.state.operator.public_url().unwrap().origin());

          assert_eq!(api.dump(), admin.dump(), "{target}");
          assert_eq!(
              api.state.operator.public_url(),
              admin.state.operator.public_url(),
              "{target}"
          );
          for (c, ends) in [&api, &admin].into_iter().zip(&announced) {
              assert!(c.state.operator.ceremonies.is_empty(), "{target}");
              assert!(ends.has_changed().unwrap(), "{target}: the ending was not announced");
          }
      }
  }
  ```

Create `crates/hennery-testkit/tests/public_url_log.rs`:

  ```rust
  //! Plan 4d-B4's review, A2: a `public_url` changed over the API is logged
  //! at `warn` with the origin left, the origin taken and what the change
  //! ended. In a test binary of its own, with a global subscriber: the
  //! collector's tasks run on the runtime's threads, not the test's.

  use hennery_kernel::hosts::Hosts;
  use hennery_kernel::operator::Operator;
  use hennery_sessions::AppState;
  use hennery_sessions::store::Store;
  use hennery_testkit::PUBLIC_URL;
  use std::sync::{Arc, Mutex};

  /// A `tracing` writer into a shared buffer.
  #[derive(Clone, Default)]
  struct Captured(Arc<Mutex<Vec<u8>>>);

  impl std::io::Write for Captured {
      fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
          self.0.lock().unwrap().extend_from_slice(buf);
          Ok(buf.len())
      }

      fn flush(&mut self) -> std::io::Result<()> {
          Ok(())
      }
  }

  #[tokio::test]
  async fn a_change_over_the_api_is_logged_with_both_origins() {
      use tracing_subscriber::util::SubscriberInitExt;
      let captured = Captured::default();
      let writer = captured.clone();
      tracing_subscriber::fmt()
          .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
          .with_ansi(false)
          .with_writer(move || writer.clone())
          .finish()
          .init();

      let dir = tempfile::tempdir().unwrap();
      let db = dir.path().join("hennery.db");
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      let state = AppState::new(
          Store::open(&db).unwrap(),
          Hosts::open(&db).unwrap(),
          Operator::open(&db).unwrap(),
      );
      let client = hennery_testkit::operator_client(&state.operator);
      tokio::spawn(hennery_sessions::serve(listener, state.clone()));
      let resp = client
          .patch(format!("http://{addr}/api/settings"))
          .json(&serde_json::json!({ "public_url": "https://moved.example" }))
          .send()
          .await
          .unwrap();
      assert_eq!(resp.status(), 200);

      let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
      let line = logged
          .lines()
          .find(|line| line.contains("public_url changed over the API"))
          .unwrap_or_else(|| panic!("not logged:\n{logged}"));
      assert!(line.contains(" WARN "), "{line}");
      // `Debug`-quoted, as every string field is.
      assert!(line.contains(&format!("from=\"{PUBLIC_URL}\"")), "{line}");
      assert!(line.contains("to=\"https://moved.example\""), "{line}");
      assert!(line.contains("sessions_ended=1 passkeys_removed=0"), "{line}");
  }
  ```

In `crates/hennery-testkit/tests/push.rs`, replace:

  ```rust
      // `public_url` is not changed here (`hennery admin reset-public-url`).
      let resp = patch(serde_json::json!({ "public_url": "https://moved.example" }))
          .await
          .unwrap();
  ```

with:

  ```rust
      // Anything else is refused (`public_url` is `public_url.rs`'s).
      let resp = patch(serde_json::json!({ "name": "x" })).await.unwrap();
  ```


Run: `nix develop -c cargo build --workspace` (it adds `tracing-subscriber` to the testkit's entry in `Cargo.lock`).

- [ ] **Step 2: Run them, and see them fail**

Run: `nix develop -c cargo test -p hennery-testkit --test public_url --test public_url_log --locked`
Expected: `public_url` does not compile: `unresolved imports hennery_proto::rest::PublicUrlChanged, hennery_proto::rest::SettingsUpdateResponse`.

- [ ] **Step 3: Write the implementation**

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::PushKeys,
          rest::PushSubscribeRequest,
          rest::PushRotateRequest,
          rest::PushUnsubscribeRequest,
          rest::PushSubscriptionItem,
          rest::PushPolicyRequest,
          rest::PushPolicyItem,
          rest::SettingsResponse,
          rest::SettingsUpdateRequest,
          rest::PushPayload,
          rest::McpCredKind,
          rest::McpConnectionStatus,
          rest::McpConnectionItem,
          rest::CreateMcpConnectionRequest,
          rest::UpdateMcpConnectionRequest,
          rest::McpMountsRequest,
          rest::McpCredentialRequest,
          rest::DeleteResult,
  ```

with:

  ```rust
          rest::PushKeys,
          rest::PushSubscribeRequest,
          rest::PushRotateRequest,
          rest::PushUnsubscribeRequest,
          rest::PushSubscriptionItem,
          rest::PushPolicyRequest,
          rest::PushPolicyItem,
          rest::SettingsResponse,
          rest::SettingsUpdateRequest,
          rest::SettingsUpdateResponse,
          rest::PublicUrlChanged,
          rest::PushPayload,
          rest::McpCredKind,
          rest::McpConnectionStatus,
          rest::McpConnectionItem,
          rest::CreateMcpConnectionRequest,
          rest::UpdateMcpConnectionRequest,
          rest::McpMountsRequest,
          rest::McpCredentialRequest,
          rest::DeleteResult,
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::SettingsUpdateRequest,
          rest::PushPayload,
  ```

with:

  ```rust
          rest::SettingsUpdateRequest,
          rest::SettingsUpdateResponse,
          rest::PublicUrlChanged,
          rest::PushPayload,
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
  /// `GET /api/settings` (kernel spec §8), and the answer to its `PATCH`.
  ```

with:

  ```rust
  /// `GET /api/settings` (kernel spec §8).
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
  /// `PATCH /api/settings`: absent fields stay as they are. `contact` is an
  /// e-mail address, trimmed, or empty (or blank) to clear it. `public_url` is not changed here
  /// yet: `hennery admin reset-public-url` does that (kernel spec §4.2).
  ```

with:

  ```rust
  /// `PATCH /api/settings`: absent fields stay as they are, and so does a
  /// `null` one. `contact` is an e-mail address, trimmed, or empty (or blank)
  /// to clear it.
  ///
  /// `public_url` moves the collector (kernel spec §3.2), as
  /// `hennery admin reset-public-url` does (§4.2), and needs a fresh step-up
  /// (§3.4). It ends every signed-in session, this one included (its cookie
  /// is cleared), and so every stream and push subscription; it removes
  /// every passkey when the host name changes. The browser must then sign in
  /// at the new origin: requests from the old one are refused. A typo is
  /// recovered on the collector's machine, with `hennery admin
  /// reset-public-url`. A body with both fields changes both or neither.
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
  }

  /// What a push carries to the browser's service worker (kernel spec §6;
  ```

with:

  ```rust
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub public_url: Option<String>,
  }

  /// The answer to `PATCH /api/settings`: the settings as they are now and,
  /// when `public_url` was changed, what the change ended. Without
  /// `public_url_changed` it is exactly `SettingsResponse`.
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct SettingsUpdateResponse {
      pub public_url: String,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub contact: Option<String>,
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "PublicUrlChanged | undefined", optional)]
      pub public_url_changed: Option<PublicUrlChanged>,
  }

  /// What a `public_url` change ended, as `hennery admin reset-public-url`
  /// reports it: every signed-in session (the caller's own among them), and
  /// the passkeys of a host name `public_url` no longer has.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct PublicUrlChanged {
      #[ts(type = "number")]
      pub sessions_ended: u64,
      #[ts(type = "number")]
      pub passkeys_removed: u64,
  }

  /// What a push carries to the browser's service worker (kernel spec §6;
  ```

In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
  use axum::http::StatusCode;
  ```

with:

  ```rust
  use axum::http::{HeaderValue, StatusCode, header};
  ```

In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
  use hennery_kernel::operator::Authenticated;
  use hennery_kernel::push::{NewSubscription, PushPolicy, Subscribed, Subscription};
  use hennery_kernel::secret::{rfc3339, unix_now};
  use hennery_proto::rest::{
      PushPolicyItem, PushPolicyRequest, PushRotateRequest, PushSubscribeRequest, PushSubscriptionItem,
      PushUnsubscribeRequest, SettingsResponse, SettingsUpdateRequest, VapidKeyResponse,
  ```

with:

  ```rust
  use hennery_kernel::operator::{Authenticated, PublicUrl, PublicUrlChange, cleared_cookie};
  use hennery_kernel::push::{NewSubscription, PushPolicy, Subscribed, Subscription};
  use hennery_kernel::secret::{rfc3339, unix_now};
  use hennery_proto::rest::{
      PublicUrlChanged, PushPolicyItem, PushPolicyRequest, PushRotateRequest, PushSubscribeRequest, PushSubscriptionItem,
      PushUnsubscribeRequest, SettingsResponse, SettingsUpdateRequest, SettingsUpdateResponse, VapidKeyResponse,
  ```

In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
      settings_now(&state)
  }

  /// `PATCH /api/settings`: 200 with the settings as they are now. An empty
  /// `contact` clears it.
  async fn update_settings(State(state): State<AppState>, ApiJson(req): ApiJson<SettingsUpdateRequest>) -> Response {
      if let Some(contact) = req.contact {
          let contact = contact.trim();
          match state.operator.set_contact((!contact.is_empty()).then_some(contact)) {
  ```

with:

  ```rust
      match settings_now(&state) {
          Ok(settings) => Json(settings).into_response(),
          Err(err) => internal(err),
      }
  }

  /// `PATCH /api/settings`: 200 with the settings as they are now. An empty
  /// `contact` clears it. A `public_url` needs a fresh step-up, checked
  /// before anything is read or written (plan 4d-B4 decision 2), and moves
  /// the collector as `hennery admin reset-public-url` does (decision 1).
  /// A body without one needs none (kernel spec §6).
  async fn update_settings(
      State(state): State<AppState>,
      Extension(session): Extension<Authenticated>,
      ApiJson(req): ApiJson<SettingsUpdateRequest>,
  ) -> Response {
      let contact = req
          .contact
          .as_deref()
          .map(str::trim)
          .map(|contact| (!contact.is_empty()).then_some(contact));
      if let Some(public_url) = req.public_url.as_deref() {
          if !session.stepped_up(unix_now()) {
              return hennery_kernel::auth::step_up_required();
          }
          return change_public_url(&state, &session, public_url, contact);
      }
      if let Some(contact) = contact {
          match state.operator.set_contact(contact) {
  ```

In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
      settings_now(&state)
  }

  fn settings_now(state: &AppState) -> Response {
  ```

with:

  ```rust
      match settings_now(&state) {
          Ok(settings) => Json(SettingsUpdateResponse {
              public_url: settings.public_url,
              contact: settings.contact,
              public_url_changed: None,
          })
          .into_response(),
          Err(err) => internal(err),
      }
  }

  /// `public_url` moved, and with it the contact when the body names one
  /// (decision 1). The operator re-checks the caller's session in its write
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
          Ok(PublicUrlChange::Done {
              from,
              to,
              sessions_ended,
              passkeys_removed,
          }) => {
              tracing::warn!(
                  from = from.as_ref().map(PublicUrl::origin).unwrap_or_default(),
                  to = to.origin(),
                  sessions_ended,
                  passkeys_removed,
                  "public_url changed over the API"
              );
              (from, to, sessions_ended, passkeys_removed)
          }
          Ok(PublicUrlChange::Invalid(why)) => return error(StatusCode::BAD_REQUEST, "invalid", why),
          // The session ended after the request's checks (a revoke, a reset):
          // as if it had not been signed in, and its cookie is dead.
          Ok(PublicUrlChange::SignedOut) => {
              let secure = state.operator.public_url().is_none_or(|url| url.is_https());
              return with_cleared_cookie(
                  error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first"),
                  secure,
              );
          }
          Ok(PublicUrlChange::StepUpRequired) => return hennery_kernel::auth::step_up_required(),
          // Unreachable here: the browser rules refuse every state-changing
          // request before setup.
          Ok(PublicUrlChange::NotSetUp) => {
              return error(StatusCode::FORBIDDEN, "setup_required", "hennery is not set up yet");
          }
          Err(err) => return internal(err),
      };
      let response = match state.operator.contact() {
          Ok(contact) => Json(SettingsUpdateResponse {
              public_url: to.origin().to_string(),
              contact,
              public_url_changed: Some(PublicUrlChanged {
                  sessions_ended: sessions_ended as u64,
                  passkeys_removed: passkeys_removed as u64,
              }),
          })
          .into_response(),
          Err(err) => internal(err),
      };
      with_cleared_cookie(response, from.as_ref().is_none_or(PublicUrl::is_https))
  }

  /// `response` with the session cookie cleared: whatever it answers, the
  /// session has ended.
  fn with_cleared_cookie(mut response: Response, secure: bool) -> Response {
      if let Ok(cookie) = HeaderValue::from_str(&cleared_cookie(secure)) {
          response.headers_mut().append(header::SET_COOKIE, cookie);
      }
      response
  }

  fn settings_now(state: &AppState) -> anyhow::Result<SettingsResponse> {
  ```

In `crates/hennery-sessions/src/push.rs`, replace:

  ```rust
      match state.operator.contact() {
          Ok(contact) => Json(SettingsResponse { public_url, contact }).into_response(),
          Err(err) => internal(err),
      }
  ```

with:

  ```rust
      let contact = state.operator.contact()?;
      Ok(SettingsResponse { public_url, contact })
  ```


Run: `nix develop -c cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run the tests, and see them pass**

Run: `nix develop -c cargo test -p hennery-testkit --test public_url --test public_url_log --test push --test step_up --test owner_filter --locked`
Expected: all pass.

Run: `nix develop -c bash -c 'pnpm --dir web install --frozen-lockfile && pnpm --dir web typecheck && pnpm --dir web test'`
Expected: all pass (the TypeScript gains the types; nothing in `web/src` uses them yet).

- [ ] **Step 5: Revert-probes**

Each on this task's tree; each must make a test fail, then be restored. The probes of Task 1's lines are run again here, against the HTTP tests alone, so the API's tests are shown to catch them too.
- P11: In `push.rs`, pass no caller: `let caller = None::<(&str, i64)>;`. Fails: `a_session_revoked_while_its_body_arrives_changes_nothing`. Restore it.
- P12: Remove the handler's `if !session.stepped_up(unix_now())` check (the write's own check still answers 403, but only after the URL is parsed, so an invalid URL from a stale session is 400). Fails: `naming_public_url_needs_a_fresh_step_up_and_a_contact_alone_does_not`. Restore it.
- P13: Answer `response` without `with_cleared_cookie`. Fails: `the_cookie_is_cleared_as_the_old_public_url_set_it`, `the_same_origin_again_ends_every_session_and_keeps_the_passkeys`, `a_change_ends_every_session_stream_subscription_and_ceremony`. Restore it.
- P14: Take the `Secure` flag from `to.is_https()` instead of `from`. Fails: `the_cookie_is_cleared_as_the_old_public_url_set_it`. Restore it.
- P15: Answer the `SignedOut` 401 without clearing the cookie. Fails: `a_session_revoked_while_its_body_arrives_changes_nothing`. Restore it.
- P16: Log at `debug` instead of `warn`. Fails: `a_change_over_the_api_is_logged_with_both_origins`. Restore it.
- P17: Answer `Invalid` with 422 instead of 400. Fails: `every_refusal_answers_its_code_and_changes_nothing`. Restore it.
- P18: Pass `None` for the contact instead of the body's. Fails: `every_refusal_answers_its_code_and_changes_nothing`. Restore it.
- P19: Answer `SignedOut` with 403 instead of 401. Fails: `a_session_revoked_while_its_body_arrives_changes_nothing`. Restore it.
- P22: Answer the write's `StepUpRequired` with 401 `unauthenticated` instead of `step_up_required()`. Fails: `a_step_up_lapsed_while_its_body_arrives_changes_nothing`. Restore it.
- P23: In `rest.rs`, remove `#[serde(deny_unknown_fields)]` from `SettingsUpdateRequest` (another case, or a stray space, would be a key ignored). Fails: `naming_public_url_needs_a_fresh_step_up_and_a_contact_alone_does_not`, `every_refusal_answers_its_code_and_changes_nothing`. Restore it.

Task 1's probes P2–P8, run against this task's tests too, are each caught by `public_url.rs` (as listed under Task 1); P1 by the kernel's test only, P9 and P10 by both.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run: `nix develop -c cargo test -p hennery-testkit --test public_url --no-run --locked`, then run the printed binary four times at once, three rounds.
Expected: every run passes. (`a_session_revoked_while_its_body_arrives_changes_nothing` and `a_step_up_lapsed_while_its_body_arrives_changes_nothing` wait for a positive signal, the session's slide, before they revoke it or step it down.)

- [ ] **Step 7: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **1412 tests**.

- [ ] **Step 8: Commit**

```bash
git add Cargo.lock crates schema web/src/generated
git commit -m "feat(settings): PATCH /api/settings moves public_url behind a fresh step-up"
```

### Task 3: The spec write-back

**Files:**
- Modify: `docs/specs/2026-09-26-kernel-design.md` (§3.2, §3.4, §4.2, §6, §8), `docs/specs/2026-09-25-hennery-architecture-design.md` (§7.5), `docs/specs/2026-09-26-frontend-design.md` (§8)

- [ ] **Step 1: Write the specs' new text**

In `docs/specs/2026-09-25-hennery-architecture-design.md`, replace:

  ```markdown
    warns before saving. A collector moved after setup is re-pointed without a
    browser by `hennery admin reset-public-url` (kernel §4.2), which ends every
    session.
  ```

with:

  ```markdown
    warns before saving, needs step-up, and ends every session with the
    change. A collector moved after setup, or a wrong value saved in Settings,
    is re-pointed without a browser by `hennery admin reset-public-url` (kernel
    §4.2), which does the same.
  ```

In `docs/specs/2026-09-26-frontend-design.md`, replace:

  ```markdown
    changes), optional owner contact for push (kernel spec
  ```

with:

  ```markdown
    changes; that every device signs out, this one included; and that a wrong
    value locks every browser out until `hennery admin reset-public-url` is run
    on the collector's machine. The new value is shown as an origin and typed
    twice, sent only when it changed, behind step-up; after the 200 the page
    goes to the new origin to sign in, kernel spec §3.2), optional owner
    contact for push (kernel spec
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
    saving, and the change requires step-up (§3.4).

  ```

with:

  ```markdown
    saving, and the change requires step-up (§3.4).
    - `PATCH /api/settings {public_url}` and `hennery admin reset-public-url`
      (§4.2) run one operator function, in one transaction: the value checked
      as setup checks it, the row and the cached origin replaced, every
      session ended (the caller's too) and so every stream and push
      subscription, the passkeys removed on a host-name change, every
      ceremony ended. The same origin again is a change like any other. A
      `contact` in the same body is set in the same transaction, or nothing
      is.
    - The API's write re-checks the caller's session, live and stepped up,
      as its first statement: a session revoked or a password reset after the
      request's checks changes nothing (401 `unauthenticated`, cookie cleared;
      or 403 `step_up_required`).
    - It answers 200 with the settings and `public_url_changed
      {sessions_ended, passkeys_removed}`, and clears the session cookie as
      the old `public_url` set it (`Secure` or not), since the answer goes to
      the old origin. It is logged at `warn` with the old and new origins.
    - From then on the old origin is refused (§3.3), so a wrong value locks
      every browser out; the way back is `hennery admin reset-public-url` on
      the collector's machine. A request already past the `Origin` check when
      the change lands still runs (one in-flight request per session; recorded,
      not closed). So does a password login checked before it: its session
      outlives "every session ended", and proves the password anyway.

  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  sessions, registering and removing passkeys, and the gateway's connections
  (creating, deleting, setting a static credential, and a `PATCH` naming the
  URL, kind, internal flag, header or prefix; plan 8a); the other actions get it
  with their endpoints.
  ```

with:

  ```markdown
  sessions, registering and removing passkeys, the gateway's connections
  (creating, deleting, setting a static credential, and a `PATCH` naming the
  URL, kind, internal flag, header or prefix; plan 8a), and changing
  `public_url` (a `PATCH /api/settings` naming it, checked before anything is
  read and again in its write; plan 4d-B4); the other actions get it with their
  endpoints.
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
    reports how many it removed.
  ```

with:

  ```markdown
    reports how many it removed. Settings changes `public_url` the same way
    (`PATCH /api/settings`, step-up); this reset is the way back when a wrong
    value there locks every browser out.
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
    it (no step-up), trimmed; empty clears it. It is a plain e-mail address
  ```

with:

  ```markdown
    it (no step-up, unless the body names `public_url` too, §3.2), trimmed;
    empty clears it. It is a plain e-mail address
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  | `GET/PATCH /api/settings` | `{public_url, contact}`; PATCH takes `contact` only (§6) |
  ```

with:

  ```markdown
  | `GET/PATCH /api/settings` | `{public_url, contact}`; PATCH takes `{contact?, public_url?}`: `public_url` needs step-up and ends every session (§3.2), `contact` alone none (§6) |
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  `/api/settings`, `POST /api/setup` and the setup page. `public_url` changes
  only through `hennery admin reset-public-url` (§4.2): a `public_url` in
  `PATCH /api/settings` is 422 `invalid_body`. No `/api/capabilities`,
  ```

with:

  ```markdown
  `/api/settings` (its `PATCH` taking `public_url` since plan 4d-B4),
  `POST /api/setup` and the setup page. No `/api/capabilities`,
  ```


- [ ] **Step 2: Check nothing still says otherwise**

Run: `git grep -n "PATCH takes \`contact\` only\|is 422 \`invalid_body\`. No" -- docs/specs`
Expected: nothing.

- [ ] **Step 3: Commit**

```bash
git add docs/specs
git commit -m "docs(spec): Settings changes public_url as the admin socket does"
```

## After this plan

**What the frontend must do (plan 4d):**
- **Settings' `public_url`:** the warnings and the confirmation of decision 5 (frontend spec §8). Send `public_url` only when it changed (decision 3). On 403 `step_up_required`, step up and retry, as the transport already does.
- **After the 200:** every session has ended and the cookie is cleared; go to the answer's `public_url` to sign in. Report `sessions_ended` and `passkeys_removed`.
- **The answers:** 400 `invalid` names what is wrong with the URL or the contact; nothing changed.

**Obligations this plan hands on:**
- **OAuth** (the gateway's OAuth plan, 8e): a `public_url` change changes the redirect URI (kernel §3.2); registrations made under the old one must be redone or refused.
- **3b-ii D12, for every other handler:** an epoch on the cached origin, re-checked when each handler commits, would close the race decision 7 records.
- **`warn_if_public_url_differs`** (the configured `public_url` against the stored one) names only `hennery admin reset-public-url`; Settings is now a second way.

**Not tested here:**
- **The handler's `NotSetUp` answer:** the browser rules refuse first (`before_setup_the_browser_rules_answer_first`). The operator's `NotSetUp` is tested in Task 1 (P20).
- **`reset_public_url`'s `SignedOut | StepUpRequired` arm:** unreachable by construction; the admin socket passes no caller, and only a caller is re-checked.
- **Duplicate keys:** serde refuses a duplicate field itself (`naming_public_url_needs_a_fresh_step_up_and_a_contact_alone_does_not` asserts the 422); there is no line of ours to probe. `deny_unknown_fields`, which makes another case or a stray space no key at all, is probed (P23).
- **The `IMMEDIATE` transaction:** a sequential test cannot tell it from a deferred one.
- **A real browser** following the cleared cookie, and a password login racing the change (decision 7).

**Open for the maintainer** (with the default in place):
- **Q1. Should the API refuse a loopback `http` `public_url`?** From a remote browser it is a guaranteed lockout that only `hennery admin reset-public-url` undoes. Default: accepted, as setup and the admin socket accept it (decision 8).

**The security review's answers** (2026-10-02, on the maintainer's behalf):
1. **One transaction:** yes, with the cache and ceremonies under the same lock and no deadlock; the caller was not re-checked (A1).
2. **Conditional step-up:** before any read or write, on the parsed body only; `null` as absent accepted; the escaped-key row was a copy (A3).
3. **The lockout:** (a), the admin socket as the way back; (b) and (c) are scope changes.
4. **The same origin:** parity.
5. **Passkeys:** removed only on a host-name change, as 3c decision 9.
6. **Streams, subscriptions and races:** correct, with A1.
7. **The cookie:** correct both ways; `from` read under the lock (A2).
8. **D12:** record it, once A1 closes the `PATCH` itself; the password-login race added.
9. **The log:** acceptable; logged as soon as the move is made (A2).
10. **Tests:** A3; every side-effect line probed.

**Spec amendments** (applied in Task 3):
- kernel §3.2: what a change does over the API, the caller's re-check, the answer and the cookie, the log, the lockout and its way back, the recorded race;
- kernel §3.4: changing `public_url` among what step-up guards, built;
- kernel §4.2: Settings as a second way, the admin reset as the way back;
- kernel §6: a contact needs step-up only beside a `public_url`;
- kernel §8: the `PATCH` row and "Built so far";
- umbrella §7.5 and frontend §8: the warning and the move.

---

_Generated with Claude AI — please review before distribution._
