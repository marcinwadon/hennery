# Hats (plan 5c): sessions carry their hat Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Every session belongs to a hat, decided once at its start and kept (umbrella §8.2; ACP core §4.3):
- A start resolves its cwd on the host and matches the host's rules. The hat is stored with the canonical cwd before the host is asked to start anything. The client never names a hat.
- A resume resolves the stored cwd again, and is refused when the directory moved (`cwd_moved`) or its hat changed (`hat_mismatch`, naming both hats).
- A path whose hat is in doubt is refused (`hat_ambiguous`): a rule would cover it but for case, and would give another hat.
- The host starts an adapter only in the exact canonical directory it was sent.
- `SessionItem.hat_id`, in the list and the detail; the session list's `?hat=` filters by it (plan 6b's, wired here).
- A project is remembered under the session's stored hat (plan 6c's recents).

Re-assignment is plan 5d.

**Architecture:**
- **Kernel** (`hats.rs`): `near_miss` and `Hosts::session_hat(host, path) -> SessionHat {Decided(Resolution) | Ambiguous(PathRule)}`.
- **Sessions** (`hennery-sessions`):
  - `store.rs`:
    - migration 10: `sessions.hat_id` and `hat_rule_id`, backfilled from each host's default hat, and the `sessions_by_hat` index;
    - `create_session` takes the hat and its rule;
    - `request_resume` compares the hat atomically (`ResumeRequest::HatMismatch`);
    - `list` filters by `ListQuery.hat`, through its own statement so the index serves it.
  - `api.rs`:
    - `start_session` and `resume` resolve first;
    - `Unplaceable` turns each refusal into its answer;
    - a start on a host that is away creates no session row;
    - `?hat=` is passed to the list, and an empty one refused.
  - `hats.rs`: the hat tester refuses where a start would (the review's P1).
  - `ws.rs`: `remember_project` takes the session's stored hat, not the rules' current answer.
- **Host** (`connection.rs`): `start_session` and `resume_session` refuse a cwd that is not its own canonical form, or not a directory (`cwd_not_canonical`).
- **Wire:** `SessionItem.hat_id`.
- **Tests:**
  - the store's migration and hat check;
  - start and resume over a scripted host, including the near miss for unverified and verified rules;
  - the host refusing a symlinked cwd;
  - the list's hat filter, and recents under the stored hat;
  - every scripted host now answers `resolve_path`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md), [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md) and [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md), these sections:
- umbrella §8.2: "Sequence at start: the host resolves the typed path to its canonical form → the collector matches the rules → the hat is **stored on the session** with the canonical path → the session starts. Changing rules later does not re-bucket history. On resume the hat is re-resolved; if it differs from the stored one, the resume is refused with a clear error until the operator re-assigns the session explicitly."
- ACP core §4.1: "**Immutable after creation:** host, agent, cwd (canonical)."
- ACP core §4.3: "**Hat resolution comes first.**" and "If the re-resolved hat differs from the stored one, the resume is refused (409 `hat_mismatch`) with a message naming both hats".
- ACP core §8: `sessions(…, hat_id, …)`.
- ACP core §9: the start and resume answers; `SessionDetail`.
- kernel §5.2: "**At start:** `resolve_path` → rule match → the hat is stored on the session with the canonical cwd → `start_session`".

It builds on the executed plans [5a](2026-10-06-hats.md) and [5b](2026-10-07-resolve-path.md), and on plan 6's session list (6b) and recent projects (6c). Read their "After this plan" first. Every anchor below was taken from the tree 5b leaves on today's `main`. Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** executed 2026-10-02 (see "Execution status"); amended after the security review.

The security review of 2026-10-02, binding on the maintainer's behalf, covered 5b, 5c and 5d. It approved after amendments: B1–B3 required; P1 and P4–P7 taken; P2 and P3 recorded. It then re-confirmed the amended code: "confirmed with notes".

**How the code blocks were made and checked:**
- Every block below was generated from the amended code.
- The plan was then replayed from its own text onto `6d72a03` (`main` with 5b merged). After each task's Step 1 the tree matched the tests-only commit, and after each task the task commit, byte for byte, the generated files included.
- After every task the five checks passed: 853, 860 and 861 tests, from 852.
- The guards were revert-probed.

## Execution status (2026-10-02)

**Executed** on `main` at `6d72a03` (5b merged). The code was first built and reviewed on an older base, before plan 6's session list (6b) and recent projects (6c) merged. The stronger-model security review of 5b–5d covered that code. Then one opus implementer ported it onto `main`, an opus review checked the port against the reviewed code, and the review's fixes were made in the same task. The task commits here were then staged file by file from the port, and the plan replayed onto `6d72a03`.

| Area | As built | Why |
|---|---|---|
| Migration number | Sessions migration 10, not 8 as first written. It also adds `sessions_by_hat`, and an `EXPLAIN QUERY PLAN` test pins that the list uses it. | 6b and 6c took 8 and 9. A filter on `hat_id` without the index would scan every session of the owner. |
| `SessionItem.hat_id` (6b) | `hat_id` moved from `SessionDetail` to the list item, which the detail embeds. It is a required string, and `''` means no hat. The worst-case list item is 961 bytes, under 6b's 1 KiB. | 6b's list item came first. `''` is the contract the security review approved; with 5a's default hats it never occurs for a new session. |
| `?hat=` (6b) | Wired by this plan, replacing 6b's `hat_filter_unavailable`. It has its own statement, so the index serves it. An empty `hat=` is 400 `invalid`. | 6b landed first, so the filter came to the second to land. The port review ruled on the empty value: refused, never answered with an empty list (6b's own principle). |
| Recents (6c) | `remember_project` files a project under the session's stored hat, and skips a session with no hat. A test changes the rules between the start and `session_started`. | The rules' current answer could name a hat the session is not in. That would show a hat's recent projects to another hat. |
| The port review (opus): approve with fixes | Four fixes: the empty `?hat=` above; a host unpaired between 6b's check and the hat decision answers 400 `unknown_host`, not 404; `hat_mismatch` names a session with no hat as "no hat"; and a runtime test that another owner's session never shows under a hat filter. | The review found no security issue: the order of the checks probes only the owner's own routable host, and owner filtering holds on every new statement. |
| Test harness | The scripted hosts in `images.rs` and `projects.rs` answer `resolve_path` too. `resolve.rs` gains `next_any`, since the old acks-skipping `next` broke a 5b test that expects an `Ack`. The offline-start test in `reconcile.rs` asserts that no session row exists. | Every start now resolves its cwd through the host first. |

Checks:
- After every task the five checks passed (853, 860, 861 tests, from 852). The 9 revert-probes of the tasks' "Revert-probes" steps each failed as expected.
- The run was macOS only; ubuntu CI is the Linux check (`canonical_temp_dir`, and the e2e canonicalising `/tmp`).

Deferred, not blocking (the port review):
- `remember_project`'s `is_canonical` and empty-hat guards can no longer be reached. They are kept as defensive guards, untested.
- The list item's 64-byte reserve is spent (63 bytes left). The next field on it must account for that.
- P2, P3 and P5 for `request_failed` stay recorded (see "After this plan").

## Scope

5a and 5b hand 5c these items:
- resolve the cwd with `resolve_on_host` at start and resume;
- refuse a near miss (`hat_ambiguous`);
- compare the hat atomically in `request_resume`;
- refuse a canonical cwd that changed since the start;
- store the deciding rule for audit;
- the host's own canonical-cwd check at attach;
- the hat tester refusing like a start (P1);
- `resolve_hat`'s error mapped before it can be a 500.

Plans A and B1 hand on "re-resolution on resume, `hat_mismatch`".

That is **3 tasks**:
1. the hat a session gets (kernel);
2. sessions carry their hat (store and API);
3. the host's canonical-cwd check.

**Out:**
- re-assignment and the reconciliation close (5d);
- a `hat` field on `start_session` (no host-side use until the gateway's composed `CODEX_HOME`);

## Decisions this plan makes where the spec is silent

A stronger-model security review, made on the maintainer's behalf on 2026-10-02, confirmed these: "approve after amendments", then "confirmed with notes". Items marked **(amendment)** depart from explicit spec text.

**What the review changed (for 5c):**
- B2: the near miss counts verified rules too (decision 3). A verified rule holds the case on disk only when it was saved: rename `Acme` to `acme`, and its sessions would fall silently to another hat.
- B3: the spec write-back ("Spec amendments").
- P1: `POST /api/hats/resolve` answers `hat_ambiguous` where a start would (decision 3).
- P4: a rule that names the hat that decided anyway is no near miss (decision 3).

1. **Migration 10 (the sessions component).**
   - It adds `sessions.hat_id TEXT NOT NULL DEFAULT ''` and `hat_rule_id TEXT`, and the index `sessions_by_hat(owner_id, hat_id, last_event_at DESC, id DESC)` for the list's filter.
   - The backfill takes the host's default hat; failing that, the owner's default for new hosts (a host that is gone); failing that, `''`.
   - Sessions-store tables keep no foreign keys to kernel tables, as `owner_id` in 3b-iii. Moreover, 5a's tables cannot be rebuilt while foreign keys are on.
   - A `''` hat can never resume: its re-resolved hat always differs, so `hat_mismatch` holds until the session is re-assigned.
2. **A start resolves first, and an offline host gets no session.** (amendment, ACP core §9)
   - The sequence:
     1. `resolve_on_host` of the typed cwd; it must be a directory (400 `invalid_cwd`);
     2. `session_hat` on the canonical cwd;
     3. only then `create_session` with the canonical cwd, the hat and the deciding rule;
     4. then `start_session`, carrying the canonical cwd.
   - A host that is not connected and reconciled answers 409 `host_offline`, with **no session row**: with no resolution there is no hat, and a row without one would break umbrella §8.2.
   - A `hat_id` in the request body is not a field, and is ignored.
   - Plan 6b's checks (the agent's name, `unknown_host`) still come before the host is asked anything. A host unpaired between that check and the hat decision answers the same 400 `unknown_host`.
3. **The near miss: 409 `hat_ambiguous`.** (amended after the security review of 2026-10-02: B2, P1, P4)
   - The rule: a rule that does not cover the canonical cwd byte for byte, but would after lowercasing both; that would win (a longer prefix than whatever decided); and that names another hat.
   - Verified rules count too (B2). A rule naming the deciding hat does not (P4).
   - The answer names the rule's prefix and says to save the host's rules again with the path as it is now.
   - The way out works on both kinds of volume. On a case-insensitive one, saving the rules again resolves the prefix to the directory's current case. On a case-sensitive one, an exact rule for the other directory decides at equal length.
   - The hat tester answers the same (P1).
4. **A resume re-resolves.**
   - `agent_has_no_record` comes first, without contacting the host (ACP core §4.3).
   - Then the stored cwd is resolved on the host:
     - not a directory: 400 `invalid_cwd`;
     - a canonical form other than the stored one: 409 `cwd_moved`.
   - That covers a symlink swapped in since. It also covers a session from before 5c, whose cwd was stored as typed; accepted, since nothing is released.
   - Then the near miss, then the hat. `request_resume` compares it with the stored hat in its transaction: 409 `hat_mismatch`, naming both hats ("no hat" for a session that got none), and nothing changes.
5. **The host's own check.**
   - At `start_session` and `resume_session` the host resolves the cwd again. It refuses one that does not resolve to itself as a directory (`cwd_not_canonical`), and spawns nothing.
   - A start fails with that code (`Undo::Start`), answered 502.
   - This narrows the gap between the collector's resolution and the spawn to the host's own check.
6. **`SessionItem.hat_id`, and the list's filter.** (6b landed first, refusing `?hat=` with `hat_filter_unavailable`; this plan wires it.)
   - `hat_id` is a required string on the list item, so on the list and the detail; `''` is a session from before hats that got none. The worst-case list item stays under 1 KiB (961 bytes).
   - `?hat=<id>` lists the owner's sessions in that hat, with or without `q` and `lifecycle`. An unknown hat lists nothing.
   - An empty `?hat=` is 400 `invalid`: "no hat chosen" sent as `hat=` must not look like an empty list (6b: a filter it cannot honour is refused, never ignored). Confirmed by the port's review.
   - No `hat` field on the frames yet.
7. **Recents under the stored hat.** (6c)
   - `remember_project` files the project under `sessions.hat_id`, not under whatever the rules say when `session_started` arrives. Sessions with no hat are not remembered.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **No new crates;** `Cargo.lock` does not change.
- **Wire types change in Task 2** (`SessionItem.hat_id`); regenerate there.
- **Migrations:** sessions migration 10 is new. A parallel lane taking the number first means renumbering on rebase; never edit a merged migration.
- Every query names the owner (kernel §1); `store.rs` stays in the audit's `SOURCES`.
- **No Linux-only code**, and no test that reads another process's state without polling for a positive signal (fleet rule); CI's `ubuntu-latest` is the only Linux check.
- Commits follow Conventional Commits, gmail identity, unsigned. Push the feature branch after every task; never push `main`.

## Review Focus

1. **A start under a symlinked directory** (macOS `/var/folders/…`).
   - Expected: the canonical cwd is stored and sent; the host starts the adapter there.
   - Tests: Task 2 `a_start_stores_the_canonical_cwd_and_the_hat_its_rules_give`; e2e's real-host starts.
2. **A directory renamed in case, or made after its rule** (APFS).
   - Expected: 409 `hat_ambiguous`, from a start and from the hat tester, never the default hat silently.
   - Tests: Task 1 `a_near_miss_is_a_rule_that_misses_only_by_case`; Task 2 `a_start_that_cannot_resolve_its_hat_creates_no_session`.
3. **Rules changed while a session was parked.**
   - Expected: its resume is refused (`hat_mismatch`), naming both hats, until the rules agree again. A swapped symlink: `cwd_moved`.
   - Tests: Task 2 `a_resume_re_resolves_its_cwd_and_hat_and_refuses_a_change`, `a_resume_in_another_hat_than_the_sessions_is_refused_and_changes_nothing`.
4. **An upgrade with sessions already stored.**
   - Expected: each gets its host's default hat, or the owner's default when its host is gone.
   - Tests: Task 2 `the_hat_migration_gives_each_session_its_hosts_default_hat`.
5. **A start or resume frame naming a non-canonical cwd** (an old collector, a symlink swapped in between).
   - Expected: `cwd_not_canonical`, no adapter spawned.
   - Tests: Task 3 `a_start_or_resume_in_a_cwd_that_is_not_canonical_is_refused`.

6. **Choosing "no hat" in the list's hat filter** (the frontend sends `hat=`).
   - Expected: 400 `invalid`, not an empty list.
   - Tests: Task 2 `the_session_list_pages_searches_filters_and_refuses_what_it_cannot_honour`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-kernel/src/hats.rs` | `near_miss`, `session_hat` | 1 |
| `crates/hennery-sessions/src/store.rs` | Migration 10, `create_session`, `request_resume`, the hat filter, `SessionRow` | 2 |
| `crates/hennery-sessions/src/api.rs`, `hats.rs` | Start, resume, the list's `?hat=`, the hat tester | 2 |
| `crates/hennery-sessions/src/ws.rs` | Recents under the stored hat | 2 |
| `crates/hennery-proto/src/rest.rs`, generated files | `SessionItem.hat_id` | 2 |
| `crates/hennery-host/src/connection.rs` | `cwd_problem` at attach | 3 |
| Tests: `crates/hennery-sessions/tests/{store,owner,attachments}.rs`; `crates/hennery-testkit/tests/{resolve,reconcile,projects,images,e2e,ws_ingest_error,host_connection}.rs`; `crates/hennery-proto/tests/frames.rs` | | 2, 3 |

All commands run from the repository root inside the dev shell. Work on a feature branch off `main` (e.g. `feat/hats-5c`). **Reading the steps:** as in plan 5a: "Create `path`:", "Replace the whole of `path` with:", and "In `path`, replace:" followed by a block that occurs exactly once at that point, then "with:". "Run: `cargo run -p hennery-proto --bin gen`" regenerates the generated files.

---

### Task 1: The hat a session gets

**Files:**
- Modify: `crates/hennery-kernel/src/hats.rs` (with its unit test)

**Interfaces:**
- Produces: `hennery_kernel::hats::{SessionHat {Decided(Resolution), Ambiguous(PathRule)}, near_miss(rules, path, decided) -> Option<&PathRule>}` and `Hosts::session_hat(host_id, path) -> Result<Option<SessionHat>>`.
- Consumes: 5a's `resolve`, `covers` and `rules_of`.

- [ ] **Step 1: Write the failing tests**

None apart: the unit test comes with the code in Step 3.

- [ ] **Step 2: Run them to see them fail**

Nothing to run.

- [ ] **Step 3: The near miss and the session's hat**

In `crates/hennery-kernel/src/hats.rs`, replace:

```rust
    pub rule_id: Option<String>,
```

with:

```rust
    pub rule_id: Option<String>,
}

/// The hat a session's canonical cwd gets (`Hosts::session_hat`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionHat {
    Decided(Resolution),
    /// A rule would cover the path but for case, and would give another hat
    /// than what decided it (`near_miss`): the hat is in doubt, so the
    /// session does not start or resume.
    Ambiguous(PathRule),
```

In `crates/hennery-kernel/src/hats.rs`, replace:

```rust
        })
}
```

with:

```rust
        })
}

/// A rule that does not cover `path` byte for byte but would if case were
/// ignored, that would then win over `decided` (a longer prefix than its
/// rule's, or any prefix when no rule decided), and that names another hat
/// (the review's P4). A case-insensitive volume is where that happens: the
/// rule's prefix holds another case than the directory has now, because it
/// was saved before the directory existed, or the directory was renamed
/// since (the review's B2, which is why verified rules count too). Its
/// sessions would fall silently to another hat (plan 5c decision 3).
pub fn near_miss<'a>(rules: &'a [PathRule], path: &str, decided: &Resolution) -> Option<&'a PathRule> {
    let decided_len = decided
        .rule_id
        .as_ref()
        .and_then(|id| rules.iter().find(|rule| &rule.id == id))
        .map_or(0, |rule| rule.prefix.len());
    let folded_path = path.to_lowercase();
    rules
        .iter()
        .filter(|rule| rule.prefix.len() > decided_len && rule.hat_id != decided.hat_id)
        .filter(|rule| !covers(&rule.prefix, path) && covers(&rule.prefix.to_lowercase(), &folded_path))
        .max_by_key(|rule| rule.prefix.len())
}
```

In `crates/hennery-kernel/src/hats.rs`, replace:

```rust

    /// Whether one of the host's rules names a hat other than its default:
```

with:

```rust

    /// The hat a session whose canonical cwd is `path` gets on `host_id`
    /// (umbrella §8.2), unless a rule that misses only by case makes it
    /// ambiguous (`near_miss`); `None` for an unknown host. `path` must be
    /// canonical. The host's default hat and its rules are read in one
    /// transaction, so a rules change or a default change committed in
    /// between cannot mix the old one with the new (plan 5a's final review).
    pub fn session_hat(&self, host_id: &str, path: &str) -> Result<Option<SessionHat>> {
        anyhow::ensure!(is_canonical(path), "not a canonical path: {path:?}");
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(default_hat) = host_default_hat(&tx, self.owner_id(), host_id)? else {
            return Ok(None);
        };
        let rules = rules_of(&tx, self.owner_id(), host_id)?;
        tx.commit()?;
        let decided = resolve(&rules, &default_hat, path);
        Ok(Some(match near_miss(&rules, path, &decided) {
            Some(rule) => SessionHat::Ambiguous(rule.clone()),
            None => SessionHat::Decided(decided),
        }))
    }

    /// Whether one of the host's rules names a hat other than its default:
```

In `crates/hennery-kernel/src/hats.rs`, replace:

```rust
    }

    #[test]
    fn a_rule_for_the_root_covers_every_path() {
```

with:

```rust
    }

    /// Plan 5c decision 3: a rule that misses only by case, would win, and
    /// names another hat is a near miss, verified or not (the review's B2:
    /// a directory renamed since the rule was saved); a shorter one, one
    /// that matches anyway, or one of the deciding hat (P4) is not.
    #[test]
    fn a_near_miss_is_a_rule_that_misses_only_by_case() {
        let rules = [
            PathRule {
                verified: false,
                ..rule("r-acme", "/Users/me/acme", "hat-x")
            },
            rule("r-users", "/Users/me", "hat-me"),
            rule("r-renamed", "/Users/me/Other", "hat-o"),
            rule("r-same", "/Users/me/Same", "hat-me"),
            rule("r-short", "/users", "hat-x"),
        ];
        let decided = |path: &str| resolve(&rules, "d", path);
        let miss = |path: &str| near_miss(&rules, path, &decided(path)).map(|r| r.id.clone());
        assert_eq!(miss("/Users/me/Acme/x").as_deref(), Some("r-acme"));
        assert_eq!(miss("/Users/me/acme/x"), None, "it matches as it is");
        assert_eq!(miss("/Users/me/other").as_deref(), Some("r-renamed"));
        assert_eq!(miss("/Users/me/same"), None, "the same hat decides anyway");
        // `/users` is shorter than `/Users/me`, which decided: no doubt.
        assert_eq!(miss("/Users/me/x"), None);
        assert_eq!(miss("/USERS/x").as_deref(), Some("r-short"));
    }

    #[test]
    fn a_rule_for_the_root_covers_every_path() {
```

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-kernel --locked --lib near_miss`
Expected: PASS, 1 test.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `near_miss`, restore the `!rule.verified &&` filter. The test fails on `r-renamed`.
- In `near_miss`, drop `&& rule.hat_id != decided.hat_id`. The test fails on `/Users/me/same`.

- [ ] **Step 6: The full checks**

Run the five commands. Expected: all pass; **853 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-kernel
git commit -m "feat(kernel): the hat a session gets, refused on a case near miss"
```

### Task 2: Sessions carry their hat

**Files:**
- Modify: `crates/hennery-sessions/src/{store,api,hats,ws}.rs`, `crates/hennery-proto/src/rest.rs`, the generated files
- Test: `crates/hennery-sessions/tests/{store,owner,attachments}.rs`, `crates/hennery-testkit/tests/{resolve,reconcile,projects,images,e2e,ws_ingest_error}.rs`, `crates/hennery-proto/tests/frames.rs`

**Interfaces:**
- Produces:
  - sessions migration 10;
  - `Store::create_session(id, host_id, agent, cwd, hat_id, rule_id: Option<&str>)` and `Store::request_resume(session_id, hat_id)`, with `ResumeRequest::HatMismatch(String)`;
  - `SessionRow.{hat_id, agent_session_id}`, `SessionItem.hat_id` and `ListQuery.hat`;
  - `crate::api::Unplaceable`;
  - the answers 400 `invalid_cwd`, 409 `hat_ambiguous`, 409 `cwd_moved` and 409 `hat_mismatch`.
- Consumes: Task 1's `session_hat`; 5b's `resolve_on_host`; 6b's `ListQuery`; 6c's `remember_project`.

- [ ] **Step 1: Write the failing tests**

The scripted hosts in `reconcile.rs`, `ws_ingest_error.rs`, `images.rs` and `projects.rs` now answer `resolve_path`, each path to itself. `e2e.rs`'s fake connection does the same. `resolve.rs` gains `next_any`, which does not skip acks. The store's callers name a hat (`"hat-1"` in tests).

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        cwd: "/tmp".into(),
        title: None,
```

with:

```rust
        cwd: "/tmp".into(),
        hat_id: "hat-1".into(),
        title: None,
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp",
```

with:

```rust
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp", "hat_id": "hat-1",
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
/// and room left for the `hat_id` hats add (plan 6b decision 10, the
/// reviews' A1 and P2).
```

with:

```rust
/// and the `hat_id` hats added: `hat-` and 16 hex digits, as the kernel
/// mints them (plan 6b decision 10, the reviews' A1 and P2; plan 5c).
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        cwd: format!("/{}", "\"\\😀".repeat(15)) + &"x".repeat(7),
```

with:

```rust
        cwd: format!("/{}", "\"\\😀".repeat(15)) + &"x".repeat(7),
        hat_id: "hat-0123456789abcdef".into(),
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
    // `,"hat_id":"…"` with a 36-character id is 48 bytes; 64 are kept.
    assert!(json.len() <= 1024 - 64, "{} bytes: {json}", json.len());
```

with:

```rust
    assert!(json.len() <= 1024, "{} bytes: {json}", json.len());
```

In `crates/hennery-sessions/tests/attachments.rs`, replace:

```rust
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
        // 6b's migration (version 9) undone first, its index on `owner_id`
        // included.
        conn.execute_batch(
            "
```

with:

```rust
        // 5c's migration (version 10) and 6b's (version 9) undone first,
        // their indexes on `owner_id` included.
        conn.execute_batch(
            "
            DROP INDEX sessions_by_hat;
            ALTER TABLE sessions DROP COLUMN hat_id;
            ALTER TABLE sessions DROP COLUMN hat_rule_id;
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust

    // The control: the owner's world is there.
```

with:

```rust

    // Both owners' sessions in one hat, for the hat filter.
    conn.execute(
        "UPDATE sessions SET hat_id = 'hat-x' WHERE id IN ('session-a', 'session-b')",
        [],
    )
    .unwrap();

    // The control: the owner's world is there.
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
    assert!(store.list(&search).unwrap().sessions.is_empty());
```

with:

```rust
    assert!(store.list(&search).unwrap().sessions.is_empty());
    // And a hat filter naming a hat both owners' sessions carry.
    let by_hat = ListQuery {
        hat: Some("hat-x"),
        ..ListQuery::default()
    };
    let listed: Vec<String> = store
        .list(&by_hat)
        .unwrap()
        .sessions
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    assert_eq!(listed, ["session-a"]);
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
    assert_eq!(store.request_resume("session-b").unwrap(), ResumeRequest::NotFound);
```

with:

```rust
    assert_eq!(
        store.request_resume("session-b", "hat-1").unwrap(),
        ResumeRequest::NotFound
    );
```

In `crates/hennery-sessions/tests/owner.rs`, replace:

```rust
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
fn started(store: &Store) {
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
fn started(store: &Store) {
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    assert!(
```

with:

```rust
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    assert!(
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust

    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "a")).unwrap();
```

with:

```rust

    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.ingest("s2", 1, &SessionBody::session_started("r", "a")).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("lost", "h1", "fake", "/tmp").unwrap();
    store.create_session("pending", "h1", "fake", "/tmp").unwrap();
    store.create_session("other-host", "h2", "fake", "/tmp").unwrap();
```

with:

```rust
    store
        .create_session("lost", "h1", "fake", "/tmp", "hat-1", None)
        .unwrap();
    store
        .create_session("pending", "h1", "fake", "/tmp", "hat-1", None)
        .unwrap();
    store
        .create_session("other-host", "h2", "fake", "/tmp", "hat-1", None)
        .unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    // Close requested, delivery unknown, still attached.
    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    // Close requested, delivery unknown, still attached.
    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    // Close requested, and the host restarted meanwhile.
    store.create_session("s3", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    // Close requested, and the host restarted meanwhile.
    store.create_session("s3", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
            "DROP INDEX sessions_by_recency;
```

with:

```rust
            "DROP INDEX sessions_by_hat;
             ALTER TABLE sessions DROP COLUMN hat_id;
             ALTER TABLE sessions DROP COLUMN hat_rule_id;
             DROP INDEX sessions_by_recency;
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    } = store.request_resume("s1").unwrap()
```

with:

```rust
    } = store.request_resume("s1", "hat-1").unwrap()
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Busy("active".into())
```

with:

```rust
    assert_eq!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Busy("active".into())
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    );
    parked(&store, 2);
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
```

with:

```rust
    );
    parked(&store, 2);
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
    ));
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(
        store.request_resume("s1").unwrap(),
```

with:

```rust
    assert_eq!(
        store.request_resume("s1", "hat-1").unwrap(),
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.close_now("s1").unwrap();
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
```

with:

```rust
    store.close_now("s1").unwrap();
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    );
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
```

with:

```rust
    );
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store.mark_failed("s1", "start_not_delivered").unwrap();
    assert_eq!(store.request_resume("s1").unwrap(), ResumeRequest::NoRecord);
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "failed");
    assert_eq!(store.request_resume("nope").unwrap(), ResumeRequest::NotFound);
```

with:

```rust
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.mark_failed("s1", "start_not_delivered").unwrap();
    assert_eq!(store.request_resume("s1", "hat-1").unwrap(), ResumeRequest::NoRecord);
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "failed");
    assert_eq!(store.request_resume("nope", "hat-1").unwrap(), ResumeRequest::NotFound);
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust

    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
```

with:

```rust

    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    let ResumeRequest::Starting { events, .. } = store.request_resume("s1").unwrap() else {
```

with:

```rust
    let ResumeRequest::Starting { events, .. } = store.request_resume("s1", "hat-1").unwrap() else {
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
fn a_start_that_ran_after_all_revives_the_session_without_its_failure_reason() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store.reconcile_host("h1", &[]).unwrap();
```

with:

```rust
fn a_start_that_ran_after_all_revives_the_session_without_its_failure_reason() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.reconcile_host("h1", &[]).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store.reconcile_host("h1", &[]).unwrap();
```

with:

```rust
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.reconcile_host("h1", &[]).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s2", "h2", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s2", "h2", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.presume_parked("h1").unwrap();
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
```

with:

```rust
    store.presume_parked("h1").unwrap();
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    // Closed, then resumed: the late rejection must leave the start alone.
    assert!(matches!(
        store.request_resume("s1").unwrap(),
        ResumeRequest::Starting { .. }
```

with:

```rust
    // Closed, then resumed: the late rejection must leave the start alone.
    assert!(matches!(
        store.request_resume("s1", "hat-1").unwrap(),
        ResumeRequest::Starting { .. }
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.request_resume("s1").unwrap();
```

with:

```rust
    store.request_resume("s1", "hat-1").unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
        store.request_resume("s1").unwrap(),
```

with:

```rust
        store.request_resume("s1", "hat-1").unwrap(),
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
fn started_with(store: &Store, indexed: Indexed) {
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
fn started_with(store: &Store, indexed: Indexed) {
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    let ResumeRequest::Starting { config: wanted, .. } = store.request_resume("s1").unwrap() else {
```

with:

```rust
    let ResumeRequest::Starting { config: wanted, .. } = store.request_resume("s1", "hat-1").unwrap() else {
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s3", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s3", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store.create_session("s5", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s2", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    store.create_session("s5", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s4", "h2", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s4", "h2", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    let (created_at, none) = recency(&store);
```

with:

```rust
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
    let (created_at, none) = recency(&store);
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
```

with:

```rust
    store.create_session("s1", "h1", "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    store.create_session("s1", "h1", "fake", &cwd).unwrap();
```

with:

```rust
    store.create_session("s1", "h1", "fake", &cwd, "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
            cwd,
```

with:

```rust
            cwd,
            hat_id: "hat-1".into(),
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
        store.create_session(id, "h1", "fake", cwd).unwrap();
```

with:

```rust
        store.create_session(id, "h1", "fake", cwd, "hat-1", None).unwrap();
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
        (Some("main"), Some(false))
    );
}
```

with:

```rust
        (Some("main"), Some(false))
    );
}

/// ACP core §4.3: a resume whose path now resolves to another hat is
/// refused with the stored hat, and nothing changes; the same hat goes on.
#[test]
fn a_resume_in_another_hat_than_the_sessions_is_refused_and_changes_nothing() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("s1", "h1", "fake", "/tmp", "hat-a", None).unwrap();
    store
        .ingest("s1", 1, &SessionBody::session_started("r1", "agent-1"))
        .unwrap();
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(
        store.request_resume("s1", "hat-b").unwrap(),
        ResumeRequest::HatMismatch("hat-a".into())
    );
    let row = store.session("s1").unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.hat_id.as_str()), ("parked", "hat-a"));
    assert!(matches!(
        store.request_resume("s1", "hat-a").unwrap(),
        ResumeRequest::Starting { .. }
    ));
}

/// Plan 5c decision 1: a session from before hats gets its host's default
/// hat; one whose host is gone, the owner's default for new hosts.
#[test]
fn the_hat_migration_gives_each_session_its_hosts_default_hat() {
    use hennery_kernel::hats::HatChange;
    use hennery_kernel::hosts::{Enrollment, Hosts};
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let HatChange::Done(acme) = hosts.create_hat("Acme", None, 1).unwrap() else {
        panic!("no hat");
    };
    let enrollment = Enrollment {
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        name: "laptop".into(),
        host_version: "0".into(),
        platform: "linux".into(),
    };
    hosts.register("h1", &enrollment, 1).unwrap();
    hosts.update_host("h1", None, Some(&acme.id)).unwrap();
    {
        let store = Store::open(&db).unwrap();
        store
            .create_session("s-on-h1", "h1", "fake", "/tmp", "x", None)
            .unwrap();
        store
            .create_session("s-gone", "h-gone", "fake", "/tmp", "x", None)
            .unwrap();
    }
    // Back to the store's schema before hats (version 9).
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP INDEX sessions_by_hat;
         ALTER TABLE sessions DROP COLUMN hat_id;
         ALTER TABLE sessions DROP COLUMN hat_rule_id;
         PRAGMA user_version = 9;",
    )
    .unwrap();
    let store = Store::open(&db).unwrap();
    assert_eq!(store.session("s-on-h1").unwrap().unwrap().hat_id, acme.id);
    assert_eq!(store.session("s-gone").unwrap().unwrap().hat_id, personal);
}

/// Plan 5c (plan 6b's "After this plan"): the list is filtered by the hat
/// a session belongs to, with the lifecycle filter, and with a search,
/// which bypasses every filter but the hat's (frontend §5); its pages
/// follow the cursor within the hat.
#[test]
fn the_list_filters_by_hat_with_or_without_a_search() {
    let store = Store::open_in_memory().unwrap();
    for (id, cwd, hat) in [
        ("s1", "/src/alpha", "hat-a"),
        ("s2", "/src/alpha-b", "hat-b"),
        ("s3", "/src/gamma", "hat-a"),
        ("s4", "/src/old", ""),
    ] {
        store.create_session(id, "h1", "fake", cwd, hat, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    store.close_now("s1").unwrap();
    let list = |hat: Option<&str>, lifecycles: Option<&[&str]>, search: Option<&str>, limit: u32| {
        store
            .list(&ListQuery {
                hat,
                lifecycles,
                search,
                limit,
                ..ListQuery::default()
            })
            .unwrap()
    };
    let page = list(Some("hat-a"), None, None, 50);
    assert_eq!(ids(&page), ["s1", "s3"]);
    assert!(page.sessions.iter().all(|s| s.hat_id == "hat-a"));
    assert_eq!(ids(&list(Some("hat-a"), Some(&["starting"]), None, 50)), ["s3"]);
    assert_eq!(
        ids(&list(Some("hat-a"), Some(&["starting"]), Some("alpha"), 50)),
        ["s1"]
    );
    assert_eq!(ids(&list(Some("hat-b"), None, Some("alpha"), 50)), ["s2"]);
    assert_eq!(ids(&list(Some(""), None, None, 50)), ["s4"]);
    assert!(ids(&list(Some("hat-x"), None, None, 50)).is_empty());
    assert_eq!(list(None, None, None, 50).sessions.len(), 4);
    let first = list(Some("hat-a"), None, None, 1);
    assert_eq!(ids(&first), ["s1"]);
    let cursor = Cursor::decode(first.next_cursor.as_deref().unwrap()).unwrap();
    let rest = store
        .list(&ListQuery {
            after: Some(&cursor),
            limit: 1,
            hat: Some("hat-a"),
            ..ListQuery::default()
        })
        .unwrap();
    assert_eq!(ids(&rest), ["s3"]);
    assert_eq!(rest.next_cursor, None);
}
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
use hennery_kernel::operator::Operator;
```

with:

```rust
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame};
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    // Register a fake host connection directly (no real socket): once the
    // collector sends `start_session` on it, drop the connection before any
    // reply arrives, so the request resolves as DeliveryUnknown rather than
    // a rejection.
```

with:

```rust
    // Register a fake host connection directly (no real socket): it answers
    // the cwd's `resolve_path` as a host would (plan 5c), and once the
    // collector sends `start_session` on it, drops the connection before
    // any reply arrives, so the request resolves as DeliveryUnknown rather
    // than a rejection.
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        .register("host-1", tx, Default::default())
```

with:

```rust
        .register("host-1", tx, Capabilities(vec![Capability::ResolvePath]))
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let hub = collector.state.hub.clone();
    tokio::spawn(async move {
```

with:

```rust
    let hub = collector.state.hub.clone();
    let canonical = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    tokio::spawn(async move {
        if let Some(CollectorFrame::ResolvePath { request_id, .. }) = rx.recv().await {
            let answer = HostFrame::ResolvedPath {
                request_id,
                canonical: canonical.to_str().unwrap().into(),
                exists: true,
                is_dir: true,
            };
            hub.probe_reply("host-1", conn_id, answer);
        }
```

In `crates/hennery-testkit/tests/images.rs`, replace:

```rust
    async fn connect(collector: &Collector, capabilities: Capabilities) -> Self {
```

with:

```rust
    /// A host announcing `capabilities`, and `resolve_path`, which a start
    /// needs (plan 5c).
    async fn connect(collector: &Collector, mut capabilities: Capabilities) -> Self {
        capabilities.0.push(Capability::ResolvePath);
```

In `crates/hennery-testkit/tests/images.rs`, replace:

```rust
    /// The next collector frame that is not an `ack`.
```

with:

```rust
    /// The next collector frame that is not an `ack`, or a `resolve_path`:
    /// this host resolves every path to itself (plan 5c), as a host with no
    /// symlinks would.
```

In `crates/hennery-testkit/tests/images.rs`, replace:

```rust
                        CollectorFrame::Ack { .. } => {}
```

with:

```rust
                        CollectorFrame::Ack { .. } => {}
                        CollectorFrame::ResolvePath { request_id, path } => {
                            let answer = HostFrame::ResolvedPath {
                                request_id,
                                canonical: path,
                                exists: true,
                                is_dir: true,
                            };
                            self.send(&answer).await;
                        }
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust
    /// `connect` announcing the `projects` capability and no roots.
    async fn with_projects(collector: &Collector) -> Self {
        Self::connect(collector, Capabilities(vec![Capability::Projects]), vec![]).await
```

with:

```rust
    /// `connect` announcing the `projects` capability and no roots, and
    /// `resolve_path`, which a start needs (plan 5c).
    async fn with_projects(collector: &Collector) -> Self {
        Self::connect(
            collector,
            Capabilities(vec![Capability::Projects, Capability::ResolvePath]),
            vec![],
        )
        .await
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust
/// Start a session in `cwd` through the API, the host answering
/// `session_started` if `starts`, `start_failed` otherwise.
async fn start_in(collector: &Collector, host: &mut ScriptedHost, cwd: &str, starts: bool) {
    let (c, url) = (client(collector), format!("http://{}/api/sessions", collector.addr));
    let body = json!({ "host_id": HOST, "agent": "fake", "cwd": cwd });
    let call = tokio::spawn(async move { c.post(url).json(&body).send().await.unwrap().status().as_u16() });
    let CollectorFrame::StartSession {
        request_id, session_id, ..
```

with:

```rust
/// Start a session in `cwd` through the API, the host resolving it to
/// itself (plan 5c) and answering `session_started` if `starts`,
/// `start_failed` otherwise.
async fn start_in(collector: &Collector, host: &mut ScriptedHost, cwd: &str, starts: bool) {
    let start = begin_start(collector, host, cwd, cwd).await;
    finish_start(host, start, starts).await;
}

/// A start in flight: the call, and the `start_session` the host got.
struct Starting {
    call: tokio::task::JoinHandle<u16>,
    request_id: String,
    session_id: String,
}

/// Start a session in `typed` through the API, the host resolving it to
/// `canonical`, up to the `start_session` the host gets.
async fn begin_start(collector: &Collector, host: &mut ScriptedHost, typed: &str, canonical: &str) -> Starting {
    let (c, url) = (client(collector), format!("http://{}/api/sessions", collector.addr));
    let body = json!({ "host_id": HOST, "agent": "fake", "cwd": typed });
    let call = tokio::spawn(async move { c.post(url).json(&body).send().await.unwrap().status().as_u16() });
    let CollectorFrame::ResolvePath { request_id, path } = host.next().await else {
        panic!("expected resolve_path");
    };
    assert_eq!(path, typed);
    host.send(&HostFrame::ResolvedPath {
        request_id,
        canonical: canonical.into(),
        exists: true,
        is_dir: true,
    })
    .await;
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        cwd,
        ..
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust
    };
    let body = if starts {
```

with:

```rust
    };
    assert_eq!(cwd, canonical);
    Starting {
        call,
        request_id,
        session_id,
    }
}

/// The host answers a start in flight: `session_started` if `starts`,
/// `start_failed` otherwise.
async fn finish_start(host: &mut ScriptedHost, start: Starting, starts: bool) {
    let Starting {
        call,
        request_id,
        session_id,
    } = start;
    let body = if starts {
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust
    // A start that fails, and a cwd that is not canonical, are not
    // remembered; recents are read afresh, past the cache.
    start_in(&collector, &mut host, "/p/broken", false).await;
    start_in(&collector, &mut host, "relative/dir", true).await;
    start_in(&collector, &mut host, "/p/app/", true).await;
    start_in(&collector, &mut host, "/p/../etc", true).await;
```

with:

```rust
    // A start that fails is not remembered; a typed cwd is remembered as
    // its host resolved it (plan 5c), and one the host answers in a form
    // that is not canonical starts nothing. Recents are read afresh, past
    // the cache.
    start_in(&collector, &mut host, "/p/broken", false).await;
    let start = begin_start(&collector, &mut host, "/p/../p/app/", "/p/app").await;
    finish_start(&mut host, start, true).await;
    let (c, url) = (client(&collector), format!("http://{}/api/sessions", collector.addr));
    let body = json!({ "host_id": HOST, "agent": "fake", "cwd": "/p/odd" });
    let call = tokio::spawn(async move { c.post(url).json(&body).send().await.unwrap().status().as_u16() });
    let CollectorFrame::ResolvePath { request_id, .. } = host.next().await else {
        panic!("expected resolve_path");
    };
    host.send(&HostFrame::ResolvedPath {
        request_id,
        canonical: "/p/odd/".into(),
        exists: true,
        is_dir: true,
    })
    .await;
    assert_eq!(call.await.unwrap(), 502);
```

In `crates/hennery-testkit/tests/projects.rs`, replace:

```rust

/// The review's A2: `path` is checked first, whatever the host's state.
```

with:

```rust

/// Plan 6c's A3 iii (plan 5c): a recent is remembered under the hat its
/// session was given at its start, as stored, not under what its cwd
/// resolves to when the start is confirmed: a rule saved in between
/// changes nothing.
#[tokio::test]
async fn a_recent_is_remembered_under_the_hat_its_session_was_given() {
    use hennery_kernel::hats::{HatChange, NewRule};
    let collector = Collector::start().await;
    let mut host = ScriptedHost::with_projects(&collector).await;
    let hosts = &collector.state.hosts;
    let HatChange::Done(work) = hosts.create_hat("Work", None, 0).unwrap() else {
        panic!("a hat");
    };
    let start = begin_start(&collector, &mut host, "/p/app", "/p/app").await;
    let stored = collector
        .state
        .store
        .session(&start.session_id)
        .unwrap()
        .unwrap()
        .hat_id;
    assert_eq!(stored, default_hat(&collector));
    let rule = NewRule {
        prefix: "/p".into(),
        hat_id: work.id.clone(),
        verified: true,
    };
    hosts.replace_path_rules(HOST, &[rule]).unwrap();
    finish_start(&mut host, start, true).await;
    let db = collector._dir.path().join("hennery.db");
    let remembered = wait_for("the recent", || async {
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT hat_id FROM project_recents WHERE path = '/p/app'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
    })
    .await;
    assert_eq!(remembered, stored);
}

/// The review's A2: `path` is checked first, whatever the host's state.
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        Self::hello_with(collector, attached, seq, Capabilities(vec![Capability::Park])).await
```

with:

```rust
        Self::hello_with(
            collector,
            attached,
            seq,
            Capabilities(vec![Capability::Park, Capability::ResolvePath]),
        )
        .await
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        Self::connect_with(collector, attached, seq, Capabilities(vec![Capability::Park])).await
```

with:

```rust
        Self::connect_with(
            collector,
            attached,
            seq,
            Capabilities(vec![Capability::Park, Capability::ResolvePath]),
        )
        .await
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    /// The next collector frame that is not an `ack`.
```

with:

```rust
    /// The next collector frame that is not an `ack`, or a `resolve_path`:
    /// this host resolves every path to itself (plan 5c), as a host with no
    /// symlinks would.
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
                        CollectorFrame::Ack { .. } => {}
```

with:

```rust
                        CollectorFrame::Ack { .. } => {}
                        CollectorFrame::ResolvePath { request_id, path } => {
                            let answer = HostFrame::ResolvedPath {
                                request_id,
                                canonical: path,
                                exists: true,
                                is_dir: true,
                            };
                            self.ws
                                .send(Message::text(serde_json::to_string(&answer).unwrap()))
                                .await
                                .unwrap();
                        }
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
async fn a_host_that_never_returns_after_a_collector_restart_is_presumed_offline() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(&dir.path().join("hennery.db")).unwrap();
        store.create_session("s1", HOST, "fake", "/tmp").unwrap();
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
            .unwrap();
```

with:

```rust
async fn a_host_that_never_returns_after_a_collector_restart_is_presumed_offline() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::open(&dir.path().join("hennery.db")).unwrap();
        store.create_session("s1", HOST, "fake", "/tmp", "hat-1", None).unwrap();
        store
            .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
            .unwrap();
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        store.create_session("s1", HOST, "fake", "/tmp").unwrap();
```

with:

```rust
        store.create_session("s1", HOST, "fake", "/tmp", "hat-1", None).unwrap();
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let item = collector.state.store.session_item(&session).unwrap().unwrap();
```

with:

```rust
    let item = collector.state.store.session_item(&session).unwrap().unwrap();
    // No rule: the host's default hat (umbrella §8.2).
    let hat = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            "session_id": session, "host_id": HOST, "agent": "fake", "cwd": "/tmp",
```

with:

```rust
            "session_id": session, "host_id": HOST, "agent": "fake", "cwd": "/tmp", "hat_id": hat,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let mut host = ScriptedHost::connect_with(&collector, vec![], 0, Capabilities::default()).await;
```

with:

```rust
    // It resolves paths, which a start needs (plan 5c), but cannot park.
    let mut host = ScriptedHost::connect_with(&collector, vec![], 0, Capabilities(vec![Capability::ResolvePath])).await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // A paired host that is offline still gets a session, failed as before.
```

with:

```rust
    // A paired host that is offline: 409, and no session either, since its
    // cwd cannot be resolved there (plan 5c decision 2).
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
```

with:

```rust
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")), "{body}");
    let sessions: i64 = rusqlite::Connection::open(collector._dir.path().join("hennery.db"))
        .unwrap()
        .query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sessions, 0);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
/// Pages, searches and filters; every parameter it cannot honour is
/// refused, never ignored: `hat` until sessions have hats (plan 5).
```

with:

```rust
/// Pages, searches and filters, by hat too (plan 5c); every parameter it
/// cannot honour is refused, never ignored.
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    for (id, cwd) in [("a", "/src/alpha"), ("b", "/src/beta"), ("c", "/src/gamma")] {
        store.create_session(id, HOST, "fake", cwd).unwrap();
```

with:

```rust
    for (id, cwd, hat) in [
        ("a", "/src/alpha", "hat-1"),
        ("b", "/src/beta", "hat-2"),
        ("c", "/src/gamma", "hat-1"),
    ] {
        store.create_session(id, HOST, "fake", cwd, hat, None).unwrap();
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    assert_eq!(page["sessions"][0]["lifecycle"], "closed");
```

with:

```rust
    assert_eq!(page["sessions"][0]["lifecycle"], "closed");
    assert_eq!(page["sessions"][0]["hat_id"], "hat-1");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    for (query, code) in [
        ("?hat=work", "hat_filter_unavailable"),
        ("?hat=", "hat_filter_unavailable"),
```

with:

```rust
    // The hat filters, with the lifecycle's, with a search (which bypasses
    // every filter but the hat's: frontend §5), and across pages.
    assert_eq!(ids(&list("?hat=hat-1").await.1), ["a", "c"]);
    assert_eq!(ids(&list("?hat=hat-2").await.1), ["b"]);
    assert_eq!(ids(&list("?hat=hat-1&lifecycle=starting").await.1), ["c"]);
    assert_eq!(ids(&list("?hat=hat-1&lifecycle=starting&q=ALPHA").await.1), ["a"]);
    assert!(ids(&list("?hat=hat-2&q=alpha").await.1).is_empty());
    let (_, first) = list("?hat=hat-1&limit=1").await;
    assert_eq!(ids(&first), ["a"]);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    assert_eq!(
        ids(&list(&format!("?hat=hat-1&limit=1&cursor={cursor}")).await.1),
        ["c"]
    );
    // An unknown hat has no sessions; an empty one names none.
    assert!(ids(&list("?hat=work").await.1).is_empty());

    for (query, code) in [
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        ("?lifecycle=active,bogus", "invalid"),
```

with:

```rust
        ("?lifecycle=active,bogus", "invalid"),
        ("?hat=", "invalid"),
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
//! Resolving paths through their host (kernel spec §5.2, §5.4; plan 5b):
//! `POST /api/hats/resolve` and the path rules a connected host resolves.
```

with:

```rust
//! Resolving paths through their host (kernel spec §5.2, §5.4; plans 5b
//! and 5c): `POST /api/hats/resolve`, the path rules a connected host
//! resolves, and the hat a session gets at its start and keeps at its
//! resume.
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    ws: Ws,
```

with:

```rust
    ws: Ws,
    /// The last seq sent, per session.
    seqs: std::collections::HashMap<String, u64>,
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
        let mut host = Self { ws };
```

with:

```rust
        let mut host = Self {
            ws,
            seqs: Default::default(),
        };
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    async fn next(&mut self) -> CollectorFrame {
```

with:

```rust
    /// The next collector frame.
    async fn next_any(&mut self) -> CollectorFrame {
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
        .expect("a collector frame within 10s")
```

with:

```rust
        .expect("a collector frame within 10s")
    }

    /// The next collector frame that is not an `ack`: the sessions this
    /// host runs are acked (plan 5c).
    async fn next(&mut self) -> CollectorFrame {
        loop {
            match self.next_any().await {
                CollectorFrame::Ack { .. } => {}
                frame => return frame,
            }
        }
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    assert!(matches!(host.next().await, CollectorFrame::Ack { .. }));
```

with:

```rust
    assert!(matches!(host.next_any().await, CollectorFrame::Ack { .. }));
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    host.abort();
}
```

with:

```rust
    host.abort();
}

/// How many sessions the store holds, read from the file.
fn session_count(collector: &Collector) -> i64 {
    let conn = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
    conn.query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))
        .unwrap()
}

fn start(collector: &Collector, cwd: &str) -> tokio::task::JoinHandle<(u16, Value)> {
    send(
        collector,
        "POST",
        "/api/sessions",
        json!({ "host_id": HOST, "agent": "fake", "cwd": cwd, "hat_id": "hat-chosen-by-the-client" }),
    )
}

fn rule(collector: &Collector, prefix: &str, hat_id: &str, verified: bool) {
    let stored = collector
        .state
        .hosts
        .replace_path_rules(
            HOST,
            &[NewRule {
                prefix: prefix.into(),
                hat_id: hat_id.into(),
                verified,
            }],
        )
        .unwrap();
    assert!(
        matches!(stored, hennery_kernel::hats::RulesChange::Done(_)),
        "{stored:?}"
    );
}

impl ScriptedHost {
    /// Answer the next `start_session` or `resume_session` with
    /// `session_started`: its session id and cwd.
    async fn started(&mut self) -> (String, String) {
        let (request_id, session_id, cwd) = match self.next().await {
            CollectorFrame::StartSession {
                request_id,
                session_id,
                cwd,
                ..
            }
            | CollectorFrame::ResumeSession {
                request_id,
                session_id,
                cwd,
                ..
            } => (request_id, session_id, cwd),
            other => panic!("expected a start or resume, got {other:?}"),
        };
        let body = hennery_proto::frames::SessionBody::session_started(request_id, "agent-1");
        self.emit(&session_id, body).await;
        (session_id, cwd)
    }

    async fn parked(&mut self, session_id: &str) {
        let body = hennery_proto::frames::SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        };
        self.emit(session_id, body).await;
    }

    /// Send the session's next sequenced frame.
    async fn emit(&mut self, session_id: &str, body: hennery_proto::frames::SessionBody) {
        let seq = self.seqs.entry(session_id.to_string()).or_insert(0);
        *seq += 1;
        let frame = HostFrame::Session {
            session_id: session_id.into(),
            seq: *seq,
            body,
        };
        self.send(&frame).await;
    }
}

/// Umbrella §8.2: the host resolves the typed path, the collector matches
/// the rules, the hat is stored with the canonical cwd, and only then does
/// the session start, in that cwd. Whatever hat the client names is not
/// asked for and changes nothing.
#[tokio::test]
async fn a_start_stores_the_canonical_cwd_and_the_hat_its_rules_give() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    rule(&collector, "/home/me/acme", &acme, true);
    let mut host = ScriptedHost::connect(&collector).await;

    let call = start(&collector, "~/acme/x/");
    assert_eq!(host.answer("/home/me/acme/x", true).await, "~/acme/x/");
    let (session, cwd) = host.started().await;
    assert_eq!(cwd, "/home/me/acme/x");
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!(
        (row.cwd.as_str(), row.hat_id.as_str()),
        ("/home/me/acme/x", acme.as_str())
    );
    let (status, detail) = send(&collector, "GET", &format!("/api/sessions/{session}"), Value::Null)
        .await
        .unwrap();
    assert_eq!((status, detail["hat_id"].as_str()), (200, Some(acme.as_str())));
    let conn = rusqlite::Connection::open(collector._dir.path().join("hennery.db")).unwrap();
    let rule_id: Option<String> = conn
        .query_row("SELECT hat_rule_id FROM sessions WHERE id = ?1", [&session], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(rule_id.is_some_and(|id| id.starts_with("rule-")));

    // No rule covers a sibling: the host's default hat, and no rule.
    let call = start(&collector, "/home/me/acme-infra");
    host.answer("/home/me/acme-infra", true).await;
    let (session, _) = host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
    let row = collector.state.store.session(&session).unwrap().unwrap();
    let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!(row.hat_id, default);
}

/// Plan 5c decision 2: a start is refused before any session exists when
/// its host is away, its cwd is not a directory there, or its hat is in
/// doubt (decision 3).
#[tokio::test]
async fn a_start_that_cannot_resolve_its_hat_creates_no_session() {
    let collector = Collector::start().await;
    let (status, body) = start(&collector, "/p").await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));

    let mut host = ScriptedHost::connect(&collector).await;
    let call = start(&collector, "/p/file");
    host.answer("/p/file", false).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid_cwd")), "{body}");

    // A rule whose case the directory no longer has: saved before the
    // directory was made (unverified), or verified and renamed since (the
    // review's B2). The hat tester agrees with the start (P1).
    let acme = collector.hat("Acme");
    for verified in [false, true] {
        rule(&collector, "/Users/me/acme", &acme, verified);
        let call = start(&collector, "/Users/me/Acme");
        host.answer("/Users/me/Acme", true).await;
        let (status, body) = call.await.unwrap();
        assert_eq!((status, body["code"].as_str()), (409, Some("hat_ambiguous")), "{body}");
        assert!(body["message"].as_str().unwrap().contains("/Users/me/acme"), "{body}");
        let call = resolve(&collector, "/Users/me/Acme");
        host.answer("/Users/me/Acme", true).await;
        let (status, body) = call.await.unwrap();
        assert_eq!((status, body["code"].as_str()), (409, Some("hat_ambiguous")), "{body}");
    }
    assert_eq!(session_count(&collector), 0);
}

/// ACP core §4.3: a resume re-resolves its cwd on the host and its hat.
/// Another hat refuses it (`hat_mismatch`, naming both); a cwd that now
/// resolves elsewhere refuses it (`cwd_moved`, plan 5c decision 4).
/// Nothing changes either way, and the same hat resumes.
#[tokio::test]
async fn a_resume_re_resolves_its_cwd_and_hat_and_refuses_a_change() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    let call = start(&collector, "/home/me/acme");
    host.answer("/home/me/acme", true).await;
    let (session, _) = host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
    host.parked(&session).await;
    wait_for("parked", || async {
        let row = collector.state.store.session(&session).unwrap().unwrap();
        (row.lifecycle == "parked").then_some(())
    })
    .await;
    let resume = || {
        send(
            &collector,
            "POST",
            &format!("/api/sessions/{session}/resume"),
            json!({}),
        )
    };

    let acme = collector.hat("Acme");
    rule(&collector, "/home/me", &acme, true);
    let call = resume();
    assert_eq!(host.answer("/home/me/acme", true).await, "/home/me/acme");
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("hat_mismatch")), "{body}");
    let message = body["message"].as_str().unwrap();
    assert!(
        message.contains("\"Personal\"") && message.contains("\"Acme\""),
        "{message}"
    );

    collector.state.hosts.replace_path_rules(HOST, &[]).unwrap();
    let call = resume();
    host.answer("/srv/elsewhere", true).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("cwd_moved")), "{body}");
    let call = resume();
    host.answer("/home/me/acme", false).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid_cwd")), "{body}");
    assert_eq!(
        collector.state.store.session(&session).unwrap().unwrap().lifecycle,
        "parked"
    );

    let call = resume();
    host.answer("/home/me/acme", true).await;
    let (_, cwd) = host.started().await;
    assert_eq!(cwd, "/home/me/acme");
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
}
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();
```

with:

```rust
    store
        .create_session("s1", "host-1", "fake", "/tmp", "hat-1", None)
        .unwrap();
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
            capabilities: Capabilities::default(),
```

with:

```rust
            // It starts a session, so it resolves paths (plan 5c).
            capabilities: Capabilities(vec![Capability::ResolvePath]),
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust

    // Only once `create_session` has committed and the request is on its
```

with:

```rust

    // The cwd is resolved on the host first (plan 5c): to itself.
    match stream.next().await {
        Some(Ok(Message::Text(t))) => match serde_json::from_str::<CollectorFrame>(&t).unwrap() {
            CollectorFrame::ResolvePath { request_id, path } => sink
                .send(Message::text(
                    serde_json::to_string(&HostFrame::ResolvedPath {
                        request_id,
                        canonical: path,
                        exists: true,
                        is_dir: true,
                    })
                    .unwrap(),
                ))
                .await
                .unwrap(),
            other => panic!("expected resolve_path, got {other:?}"),
        },
        other => panic!("expected resolve_path frame, got {other:?}"),
    }

    // Only once `create_session` has committed and the request is on its
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-sessions --locked --test store`
Expected: FAIL to compile: `struct ListQuery<'_> has no field named hat`, `struct SessionItem has no field named hat_id`, `no variant, associated function, or constant named HatMismatch found for enum ResumeRequest`, and `create_session`/`request_resume` taking fewer arguments.

- [ ] **Step 3: The store and the routes**

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    pub agent: String,
    pub cwd: String,
    /// The title the agent reported, on one line and capped.
```

with:

```rust
    pub agent: String,
    /// Canonical on its host since hats (umbrella §8.2).
    pub cwd: String,
    /// The hat the session belongs to (umbrella §8.2), decided at its
    /// start: a hat's id, or empty for a session from before hats whose
    /// host and owner had no default hat to give it.
    pub hat_id: String,
    /// The title the agent reported, on one line and capped.
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use crate::hub::{RequestError, Undo};
```

with:

```rust
use crate::hub::{RequestError, Undo};
use crate::resolve::{NotResolved, OnHost, resolve_on_host};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use futures::stream::{self, Stream, StreamExt};
```

with:

```rust
use futures::stream::{self, Stream, StreamExt};
use hennery_kernel::hats::{Resolution, SessionHat};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust

async fn start_session(State(state): State<AppState>, ApiJson(req): ApiJson<StartSessionRequest>) -> Response {
```

with:

```rust

/// Why a session cannot start or resume where it would, and the answer
/// each gives.
pub(crate) enum Unplaceable {
    /// Its cwd did not resolve on its host.
    NotResolved(NotResolved),
    /// 400 `invalid_cwd`: the canonical cwd is not a directory there.
    NotADirectory(String),
    /// 409 `hat_ambiguous`: a rule (this prefix) that misses only by case makes its hat
    /// doubtful (plan 5c decision 3).
    Ambiguous(String),
    /// 404: no such host.
    UnknownHost,
    Internal(anyhow::Error),
}

impl IntoResponse for Unplaceable {
    fn into_response(self) -> Response {
        match self {
            Self::NotResolved(why) => why.into_response(),
            Self::NotADirectory(cwd) => error(
                StatusCode::BAD_REQUEST,
                "invalid_cwd",
                format!("{cwd:?} is not a directory on that host"),
            ),
            Self::Ambiguous(prefix) => error(
                StatusCode::CONFLICT,
                "hat_ambiguous",
                format!(
                    "the rule for {prefix:?} matches this path only in another case (the directory was made or renamed since the rule was saved): save the host's rules again with the path as it is now"
                ),
            ),
            Self::UnknownHost => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
            Self::Internal(err) => internal(err),
        }
    }
}

/// The hat a session whose canonical cwd is `cwd` gets on `host_id`
/// (umbrella §8.2), unless a rule that misses only by case makes it doubtful.
fn session_hat(state: &AppState, host_id: &str, cwd: &str) -> Result<Resolution, Unplaceable> {
    match state.hosts.session_hat(host_id, cwd) {
        Ok(Some(SessionHat::Decided(resolution))) => Ok(resolution),
        Ok(Some(SessionHat::Ambiguous(rule))) => Err(Unplaceable::Ambiguous(rule.prefix)),
        Ok(None) => Err(Unplaceable::UnknownHost),
        Err(err) => Err(Unplaceable::Internal(err)),
    }
}

/// A cwd as its host resolved it, if it is a directory there.
async fn session_cwd(state: &AppState, host_id: &str, cwd: &str) -> Result<OnHost, Unplaceable> {
    let on_host = resolve_on_host(state, host_id, cwd)
        .await
        .map_err(Unplaceable::NotResolved)?;
    if !on_host.is_dir {
        return Err(Unplaceable::NotADirectory(on_host.canonical));
    }
    Ok(on_host)
}

/// Start a session (ACP core §4.3): its cwd is resolved on its host, its
/// hat decided by that host's rules, and both stored before the host is
/// asked to start anything (umbrella §8.2). The client never names a hat.
/// A host that is not connected and reconciled gets no session at all
/// (plan 5c decision 2).
async fn start_session(State(state): State<AppState>, ApiJson(req): ApiJson<StartSessionRequest>) -> Response {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    let session_id = uuid::Uuid::now_v7().to_string();
    if let Err(err) = state
        .store
        .create_session(&session_id, &req.host_id, &req.agent, &req.cwd)
    {
```

with:

```rust
    let cwd = match session_cwd(&state, &req.host_id, &req.cwd).await {
        Ok(on_host) => on_host.canonical,
        Err(why) => return why.into_response(),
    };
    let hat = match session_hat(&state, &req.host_id, &cwd) {
        Ok(hat) => hat,
        // Unpaired since the check above: the same answer as that check.
        Err(Unplaceable::UnknownHost) => {
            return error(
                StatusCode::BAD_REQUEST,
                "unknown_host",
                "no host is paired with that id",
            );
        }
        Err(why) => return why.into_response(),
    };
    let session_id = uuid::Uuid::now_v7().to_string();
    if let Err(err) = state.store.create_session(
        &session_id,
        &req.host_id,
        &req.agent,
        &cwd,
        &hat.hat_id,
        hat.rule_id.as_deref(),
    ) {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        cwd: req.cwd,
```

with:

```rust
        cwd,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// `q` searches the title, cwd, branch and id of every session.
async fn list_sessions(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
    // Sessions have no hat until hats (plan 5) gives them `hat_id`; a
    // filter that cannot be honoured is refused, never ignored. The seam
    // hats fills.
    if params.hat.is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "hat_filter_unavailable",
            "sessions have no hat yet, so the list cannot be filtered by one",
        );
    }
```

with:

```rust
/// `q` searches the title, cwd, branch and id of every session; and only
/// the sessions of `hat`, with or without `q` (frontend §5; plan 5c). The
/// hat is matched as given: an empty one lists the sessions from before
/// hats that got none.
async fn list_sessions(State(state): State<AppState>, Query(params): Query<ListParams>) -> Response {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    }
    let query = ListQuery {
```

with:

```rust
    }
    // An empty hat names none: refused, so that "no hat chosen" sent as
    // `hat=` is not answered with an empty list (6b: a filter it cannot
    // honour is refused, never ignored).
    if params.hat.as_deref() == Some("") {
        return error(StatusCode::BAD_REQUEST, "invalid", "hat names no hat");
    }
    let query = ListQuery {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        search,
```

with:

```rust
        search,
        hat: params.hat.as_deref(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// Resume a parked, closed or failed session (ACP core §4.3): 202 with the
/// lifecycle once the host's `session_started` is ingested.
```

with:

```rust
/// `hat_id` for a message: by its name, by its id when it has none, or "no
/// hat" for a session from before hats that got none.
fn hat_name(state: &AppState, hat_id: &str) -> String {
    if hat_id.is_empty() {
        return "no hat".to_string();
    }
    match state.hosts.hat(hat_id) {
        Ok(Some(hat)) => format!("hat {:?}", hat.name),
        _ => format!("hat {hat_id}"),
    }
}

/// Resume a parked, closed or failed session (ACP core §4.3): 202 with the
/// lifecycle once the host's `session_started` is ingested. Its cwd is
/// resolved on its host again first, and must still be the same directory
/// (409 `cwd_moved`, plan 5c decision 4), and its hat re-resolved: a
/// different one refuses the resume (409 `hat_mismatch`) until the session
/// is re-assigned.
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    let (agent_session_id, committed_seq, config) = match state.store.request_resume(&id) {
```

with:

```rust
    // Before the host is asked anything (ACP core §4.3).
    if session.agent_session_id.is_none() {
        return no_record();
    }
    let on_host = match session_cwd(&state, &session.host_id, &session.cwd).await {
        Ok(on_host) => on_host,
        Err(why) => return why.into_response(),
    };
    if on_host.canonical != session.cwd {
        return error(
            StatusCode::CONFLICT,
            "cwd_moved",
            format!(
                "the session's directory {:?} now resolves to {:?} on its host; start a new session there",
                session.cwd, on_host.canonical
            ),
        );
    }
    let hat = match session_hat(&state, &session.host_id, &session.cwd) {
        Ok(hat) => hat,
        Err(why) => return why.into_response(),
    };
    let (agent_session_id, committed_seq, config) = match state.store.request_resume(&id, &hat.hat_id) {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        Ok(ResumeRequest::NoRecord) => {
            return error(
                StatusCode::CONFLICT,
                "agent_has_no_record",
                "the agent never created this session; start a new one in the same project",
```

with:

```rust
        Ok(ResumeRequest::NoRecord) => return no_record(),
        Ok(ResumeRequest::HatMismatch(stored)) => {
            return error(
                StatusCode::CONFLICT,
                "hat_mismatch",
                format!(
                    "the session belongs to {}, but its directory now resolves to {}; re-assign the session, or change the path rules",
                    hat_name(&state, &stored),
                    hat_name(&state, &hat.hat_id)
                ),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust

fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

with:

```rust

fn no_record() -> Response {
    error(
        StatusCode::CONFLICT,
        "agent_has_no_record",
        "the agent never created this session; start a new one in the same project",
    )
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
use crate::AppState;
```

with:

```rust
use crate::AppState;
use crate::api::Unplaceable;
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
use hennery_kernel::hats::{HatChange, HatRecord, MAX_RULES, NewRule, PathRule, RulesChange};
```

with:

```rust
use hennery_kernel::hats::{HatChange, HatRecord, MAX_RULES, NewRule, PathRule, RulesChange, SessionHat};
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
/// and the hat it resolves to there.
```

with:

```rust
/// and the hat a session there would get. Where a start would be refused
/// `hat_ambiguous`, so is this (the review's P1): the hat tester must agree
/// with the start.
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
    match state.hosts.resolve_hat(&req.host_id, &on_host.canonical) {
        Ok(Some(resolution)) => Json(HatResolution {
```

with:

```rust
    match state.hosts.session_hat(&req.host_id, &on_host.canonical) {
        Ok(Some(SessionHat::Ambiguous(rule))) => Unplaceable::Ambiguous(rule.prefix).into_response(),
        Ok(Some(SessionHat::Decided(resolution))) => Json(HatResolution {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
",
];
```

with:

```rust
",
    // Hats (umbrella §8.2; plan 5c decision 1): the hat a session belongs
    // to, decided once at its start, and the rule that decided it (none:
    // its host's default hat), for audit. A session from before hats gets
    // its host's default hat, or the owner's default for new hosts if its
    // host is gone; the kernel's migrations ran first (`Store::init`). The
    // list filtered by hat walks an index of its own (plan 6b's "After this
    // plan").
    "
    ALTER TABLE sessions ADD COLUMN hat_id TEXT NOT NULL DEFAULT '';
    ALTER TABLE sessions ADD COLUMN hat_rule_id TEXT;
    UPDATE sessions SET hat_id = COALESCE(
        (SELECT h.default_hat_id FROM hosts h WHERE h.id = sessions.host_id AND h.owner_id = sessions.owner_id),
        (SELECT s.value FROM settings s WHERE s.owner_id = sessions.owner_id AND s.key = 'default_hat_id'),
        '');
    CREATE INDEX sessions_by_hat ON sessions(owner_id, hat_id, last_event_at DESC, id DESC);
",
];
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    pub cwd: String,
```

with:

```rust
    /// Canonical on its host since plan 5c (umbrella §8.2).
    pub cwd: String,
    /// The hat it belongs to (umbrella §8.2), decided at its start.
    pub hat_id: String,
    /// The adapter's own session id, once a start produced one: a resume
    /// needs it (ACP core §4.3).
    pub agent_session_id: Option<String>,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

/// The outcome of `Store::request_resume`.
```

with:

```rust

/// What `request_resume` reads: lifecycle, agent session id, open turn,
/// config and hat.
type ResumeRow = (String, Option<String>, Option<String>, ConfigColumns, String);

/// The outcome of `Store::request_resume`.
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    NoRecord,
```

with:

```rust
    NoRecord,
    /// Its path now resolves to another hat than the one it belongs to
    /// (ACP core §4.3): the stored hat. Nothing changed.
    HatMismatch(String),
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
     presumed_parked, git_branch, git_dirty, model, mode, created_at, last_event_at";
```

with:

```rust
     presumed_parked, git_branch, git_dirty, model, mode, created_at, last_event_at, hat_id";
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        last_event_at: r.get(14)?,
```

with:

```rust
        last_event_at: r.get(14)?,
        hat_id: r.get(15)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    pub search: Option<&'a str>,
```

with:

```rust
    pub search: Option<&'a str>,
    /// Only sessions of this hat (plan 5c), with or without `search`
    /// (frontend §5: a search bypasses every filter but the hat's).
    pub hat: Option<&'a str>,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            search: None,
```

with:

```rust
            search: None,
            hat: None,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
/// The session list's one statement (decision 8): the owner's sessions
/// after the cursor `(?2, ?3)`, in one of the lifecycles `?4`…`?8` (a NULL
/// slot matches nothing), matching the pattern `?9` unless it is NULL, the
/// newest `last_event_at` first and the id breaking ties, at most `?10`. It
/// walks `sessions_by_recency` and sorts nothing (the review's A11).
fn list_statement() -> String {
    format!(
        "SELECT {SESSION_ITEM_COLUMNS} FROM sessions
         WHERE owner_id = ?1 AND (last_event_at, id) < (?2, ?3) AND lifecycle IN (?4, ?5, ?6, ?7, ?8)
             AND (?9 IS NULL OR title LIKE ?9 ESCAPE '\\' OR cwd LIKE ?9 ESCAPE '\\'
                  OR git_branch LIKE ?9 ESCAPE '\\' OR id LIKE ?9 ESCAPE '\\')
         ORDER BY last_event_at DESC, id DESC LIMIT ?10"
    )
```

with:

```rust
/// What every page of the session list filters on (decision 8): the
/// owner's sessions after the cursor `(?2, ?3)`, in one of the lifecycles
/// `?4`…`?8` (a NULL slot matches nothing), matching the pattern `?9`
/// unless it is NULL.
const LIST_FILTERS: &str = "owner_id = ?1 AND (last_event_at, id) < (?2, ?3) AND lifecycle IN (?4, ?5, ?6, ?7, ?8)
             AND (?9 IS NULL OR title LIKE ?9 ESCAPE '\\' OR cwd LIKE ?9 ESCAPE '\\'
                  OR git_branch LIKE ?9 ESCAPE '\\' OR id LIKE ?9 ESCAPE '\\')";

/// The session list's statement (decision 8): `LIST_FILTERS`, the newest
/// `last_event_at` first and the id breaking ties, at most `?10`; with
/// `by_hat`, only the sessions of the hat `?11` (plan 5c). It walks
/// `sessions_by_recency`, or `sessions_by_hat` for one hat, and sorts
/// nothing (the review's A11). The hat has a statement of its own: a plan
/// is made before `?11` is known, so `?11 IS NULL OR hat_id = ?11` would
/// never take the hat's index.
fn list_statement(by_hat: bool) -> String {
    if by_hat {
        format!(
            "SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE {LIST_FILTERS} AND hat_id = ?11
             ORDER BY last_event_at DESC, id DESC LIMIT ?10"
        )
    } else {
        format!(
            "SELECT {SESSION_ITEM_COLUMNS} FROM sessions WHERE {LIST_FILTERS} AND ?11 IS NULL
             ORDER BY last_event_at DESC, id DESC LIMIT ?10"
        )
    }
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    pub fn create_session(&self, id: &str, host_id: &str, agent: &str, cwd: &str) -> Result<()> {
        let ts = now();
        self.conn().execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at, owner_id)
             VALUES (?1, ?2, ?3, ?4, 'starting', ?5, ?5, ?6)",
            params![id, host_id, agent, cwd, ts, self.owner],
```

with:

```rust
    /// A new session, `starting`, in `hat_id` as `rule_id` decided (none:
    /// its host's default hat). `cwd` is canonical on its host.
    pub fn create_session(
        &self,
        id: &str,
        host_id: &str,
        agent: &str,
        cwd: &str,
        hat_id: &str,
        rule_id: Option<&str>,
    ) -> Result<()> {
        let ts = now();
        self.conn().execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, hat_id, hat_rule_id, lifecycle, created_at, last_event_at,
                                  owner_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'starting', ?7, ?7, ?8)",
            params![id, host_id, agent, cwd, hat_id, rule_id, ts, self.owner],
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        git_worktree, base_commit
```

with:

```rust
                        git_worktree, base_commit, hat_id, agent_session_id
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        cwd: r.get(3)?,
```

with:

```rust
                        cwd: r.get(3)?,
                        hat_id: r.get(18)?,
                        agent_session_id: r.get(19)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        let mut stmt = conn.prepare(&list_statement())?;
```

with:

```rust
        let mut stmt = conn.prepare(&list_statement(query.hat.is_some()))?;
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                limit + 1
```

with:

```rust
                limit + 1,
                query.hat
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    /// resume (ACP core §4.2). Atomic: of two concurrent resumes, the second
    /// sees `starting` and is refused (§12 scenario 11). A turn still open
    /// (a database written before plan B) is released first.
    pub fn request_resume(&self, session_id: &str) -> Result<ResumeRequest> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, Option<String>, Option<String>, ConfigColumns)> = tx
            .query_row(
                "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes FROM sessions
                 WHERE id = ?1 AND owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, (r.get(3)?, r.get(4)?, r.get(5)?))),
            )
            .optional()?;
        let Some((lifecycle, agent_session_id, open_turn, config)) = row else {
```

with:

```rust
    /// resume (ACP core §4.2), if its path still resolves to its own hat:
    /// `hat_id` is what it resolves to now (ACP core §4.3). Atomic: of two
    /// concurrent resumes, the second sees `starting` and is refused (§12
    /// scenario 11), and a re-assignment that lands in between is seen. A
    /// turn still open (a database written before plan B) is released
    /// first.
    pub fn request_resume(&self, session_id: &str, hat_id: &str) -> Result<ResumeRequest> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<ResumeRow> = tx
            .query_row(
                "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes, hat_id FROM sessions
                 WHERE id = ?1 AND owner_id = ?2",
                [session_id, &self.owner],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        (r.get(3)?, r.get(4)?, r.get(5)?),
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((lifecycle, agent_session_id, open_turn, config, stored_hat)) = row else {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        };
        let ts = now();
```

with:

```rust
        };
        if stored_hat != hat_id {
            return Ok(ResumeRequest::HatMismatch(stored_hat));
        }
        let ts = now();
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    /// The review's A11: the list's one statement walks the recency index,
    /// with no sort of its own, with or without a search.
```

with:

```rust
    /// The review's A11: the list's statement walks the recency index, or
    /// for one hat the hat's (plan 5c), with no sort of its own, with or
    /// without a search.
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", list_statement()))
```

with:

```rust
        for (by_hat, index) in [(false, "sessions_by_recency"), (true, "sessions_by_hat")] {
            for pattern in [Some("%x%"), None] {
                let plan = list_plan(&conn, by_hat, pattern);
                assert!(plan.contains(&format!("USING INDEX {index}")), "{plan}");
                assert!(!plan.contains("TEMP B-TREE"), "{plan}");
            }
        }
    }

    fn list_plan(conn: &Connection, by_hat: bool, pattern: Option<&str>) -> String {
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", list_statement(by_hat)))
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                    "%x%",
                    50
```

with:

```rust
                    pattern,
                    50,
                    by_hat.then_some("hat-1")
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        let plan = plan.join("\n");
        assert!(plan.contains("USING INDEX sessions_by_recency"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
```

with:

```rust
        plan.join("\n")
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
/// host's recent projects, under the hat the cwd resolves to (kernel spec
/// §5.3; plan 6c decision 11). Only a canonical cwd: until the host
/// resolves it (hats 5b), it is what the client sent. Recents are not
/// state: a failure is logged, and the fact is acked as usual.
fn remember_project(state: &AppState, host_id: &str, session_id: &str) {
    let cwd = match state.store.session(session_id) {
        Ok(Some(row)) => row.cwd,
```

with:

```rust
/// host's recent projects, under the session's hat (kernel spec §5.3; plan
/// 6c decision 11). Only a canonical cwd: a start stores the cwd as its
/// host resolved it (plan 5c), but a session from before that stored what
/// the client sent. Recents are not state: a failure is logged, and the
/// fact is acked as usual.
fn remember_project(state: &AppState, host_id: &str, session_id: &str) {
    // Under the hat the session belongs to, as stored at its start (plan
    // 6c's A3 iii), not what its cwd resolves to now: a recent is where
    // that hat's sessions ran.
    let (cwd, hat_id) = match state.store.session(session_id) {
        Ok(Some(row)) => (row.cwd, row.hat_id),
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    let hat_id = match state.hosts.resolve_hat(host_id, &cwd) {
        Ok(Some(resolution)) => resolution.hat_id,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(%host_id, %session_id, error = %err, "resolving a started session's hat failed");
            return;
        }
    };
```

with:

```rust
    // A session from before hats that got none (plan 5c decision 1).
    if hat_id.is_empty() {
        tracing::debug!(%host_id, %session_id, "not remembering the cwd of a session with no hat");
        return;
    }
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-sessions --locked --test store --test owner && cargo test -p hennery-testkit --locked --test resolve --test reconcile`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `request_resume`, remove the `stored_hat != hat_id` check. `a_resume_in_another_hat_than_the_sessions_is_refused_and_changes_nothing` and `a_resume_re_resolves_its_cwd_and_hat_and_refuses_a_change` fail.
- In `resume`, remove the `on_host.canonical != session.cwd` check. `a_resume_re_resolves_its_cwd_and_hat_and_refuses_a_change` fails on `cwd_moved`.
- In `start_session`, create the session before `session_cwd`. `a_start_that_cannot_resolve_its_hat_creates_no_session` fails: sessions exist.
- In `session_cwd`, drop the `is_dir` check. The same test fails on `invalid_cwd`.
- In `list_sessions`, drop the empty-`hat` refusal. `the_session_list_pages_searches_filters_and_refuses_what_it_cannot_honour` fails.
- In `remember_project`, file under the rules' current hat. `a_recent_is_remembered_under_the_hat_its_session_was_given` fails.

- [ ] **Step 6: The full checks**

Run the five commands. Expected: all pass; **860 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates schema web
git commit -m "feat(sessions): a session's hat is decided at its start and kept at its resume"
```

### Task 3: The host's canonical-cwd check

**Files:**
- Modify: `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Produces: the host's `error{cwd_not_canonical}` to `start_session` and `resume_session`.
- Consumes: 5b's `paths::resolve`.

- [ ] **Step 1: Write the failing tests**

The frames sent straight to a host name `std::env::temp_dir()` canonically now (`canonical_temp_dir`). On macOS it is under the `/var` symlink.

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust

fn start(request_id: &str, session_id: &str) -> CollectorFrame {
```

with:

```rust

/// The system's temporary directory in canonical form (on macOS
/// `/var/folders/…` is under a symlink): a start or resume must name its cwd
/// canonically (plan 5c decision 5).
fn canonical_temp_dir() -> String {
    std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn start(request_id: &str, session_id: &str) -> CollectorFrame {
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        agent: "fake".into(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        config: Default::default(),
```

with:

```rust
        agent: "fake".into(),
        cwd: canonical_temp_dir(),
        config: Default::default(),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
```

with:

```rust
        cwd: canonical_temp_dir(),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
    host.abort();
}
```

with:

```rust
    host.abort();
}

/// Plan 5c decision 5: the adapter starts in exactly the directory whose
/// hat was decided. A start or resume naming a cwd that is not its own
/// canonical form (a symlink, a trailing slash) or not a directory is
/// refused, and no adapter is spawned; `resolve_path` answers it.
#[tokio::test]
async fn a_start_or_resume_in_a_cwd_that_is_not_canonical_is_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("real")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
    let spawns = dir.path().join("spawns");
    tokio::spawn(run(host_with_fake(addr, "cwd-not-canonical", counting_fake(&spawns))));
    let (mut sink, mut stream, _) = accept_host(&listener).await;
    let real = std::fs::canonicalize(dir.path().join("real")).unwrap();
    let real = real.to_str().unwrap().to_string();
    let link = dir.path().join("link").to_str().unwrap().to_string();

    let cases = [
        (format!("{real}/"), "start"),
        (link.clone(), "start"),
        (format!("{real}/missing"), "start"),
        (link.clone(), "resume"),
    ];
    for (i, (cwd, kind)) in cases.iter().enumerate() {
        let request_id = format!("r{i}");
        let mut frame = if *kind == "start" {
            start(&request_id, "s1")
        } else {
            resume(&request_id, "s1", 0, "agent-1")
        };
        match &mut frame {
            CollectorFrame::StartSession { cwd: c, .. } | CollectorFrame::ResumeSession { cwd: c, .. } => {
                *c = cwd.clone()
            }
            _ => unreachable!(),
        }
        send_frame(&mut sink, &frame).await;
        match read_until(&mut stream, error_for(&request_id)).await {
            HostFrame::Error { code, .. } => assert_eq!(code, "cwd_not_canonical", "{kind} in {cwd}"),
            other => panic!("{other:?}"),
        }
    }
    assert!(!spawns.exists(), "an adapter was spawned");

    send_frame(
        &mut sink,
        &CollectorFrame::ResolvePath {
            request_id: "p1".into(),
            path: format!("{link}/"),
        },
    )
    .await;
    let answer = read_until(
        &mut stream,
        |f| matches!(f, HostFrame::ResolvedPath { request_id, .. } if request_id == "p1"),
    )
    .await;
    assert_eq!(
        answer,
        HostFrame::ResolvedPath {
            request_id: "p1".into(),
            canonical: real.clone(),
            exists: true,
            is_dir: true,
        }
    );
    // In the canonical form it names, it starts.
    let mut frame = start("r9", "s1");
    if let CollectorFrame::StartSession { cwd, .. } = &mut frame {
        *cwd = real;
    }
    send_frame(&mut sink, &frame).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test host_connection a_start_or_resume_in`
Expected: FAIL: `expected frame within 10s`: the host starts the adapter instead of refusing it.

- [ ] **Step 3: The check**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    };
    // Before anything can be enqueued for this session: continue from the
```

with:

```rust
    };
    // The adapter starts in exactly the directory whose hat the collector
    // decided (plan 5c decision 5): a cwd that no longer resolves to itself
    // (a symlink swapped in since, or one sent from before hats) is refused.
    if let Some(problem) = cwd_problem(&req.cwd) {
        uplink.reply(HostFrame::Error {
            request_id: req.request_id,
            code: "cwd_not_canonical".into(),
            message: problem,
        });
        return Ok(());
    }
    // Before anything can be enqueued for this session: continue from the
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust

/// Re-emit `session_started` from a live actor (never a second adapter), or
```

with:

```rust

/// Why `cwd` cannot be a session's directory here, if it cannot: it must be
/// a directory and in its canonical form (kernel spec §5.2).
fn cwd_problem(cwd: &str) -> Option<String> {
    match crate::paths::resolve(cwd, None) {
        Ok(resolved) if resolved.is_dir && resolved.canonical == cwd => None,
        Ok(resolved) if resolved.is_dir => Some(format!("{cwd:?} resolves to {:?} on this host", resolved.canonical)),
        Ok(_) => Some(format!("{cwd:?} is not a directory on this host")),
        Err(why) => Some(format!("{cwd:?}: {why}")),
    }
}

/// Re-emit `session_started` from a live actor (never a second adapter), or
```

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-testkit --locked --test host_connection`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

- In `cwd_problem`, accept any directory (`Ok(resolved) if resolved.is_dir => None`). `a_start_or_resume_in_a_cwd_that_is_not_canonical_is_refused` fails. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands. Expected: all pass; **861 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-host crates/hennery-testkit
git commit -m "feat(host): start an adapter only in the canonical directory its hat was decided for"
```

## After this plan

**What the frontend must do (plan 4):**
- **New session:**
  - show the hat the path resolves to (`POST /api/hats/resolve`);
  - on 409 `hat_ambiguous`, link to the host's path rules;
  - on 400 `invalid_cwd`, say the path is not a directory there;
  - on 409 `host_offline`, say no session was created.
- **Resume:**
  - 409 `hat_mismatch` names both hats: link to re-assignment (5d) and to the rules;
  - 409 `cwd_moved`: offer "start a new session there".
- **Session list and detail:** show `hat_id`'s name and colour; filter by hat with `?hat=<id>`, never `hat=`.

**Obligations this plan hands on:**
- **5d, re-assignment:** step-up; only with no running adapter; checks the hat as the owner's.
- **The gateway (plan 8):**
  - mint a session token from `sessions.hat_id` as read in the same transaction as the start or `request_resume` transition;
  - a host is mixed if `rules_name_other_hats`, or a `starting`, `active` or presumed-parked session of it has a hat other than its default: one query on `sessions.hat_id`.
- **Purge:**
  - delete the hat's sessions by `hat_id`, across every lifecycle and host;
  - refuse while any of them may run;
  - `hat_rule_id` may point at a deleted rule (audit only);
  - sessions with `hat_id = ''` belong to no hat, so list them for the operator.
- **Recorded, not done:**
  - P2: refuse a start when the typed path lexically falls under a rule that would win, but its canonical form resolves elsewhere.
  - P3: NFC folding in `near_miss`; a new crate.
  - P5 for `request_failed`: start and resume still pass host-chosen codes through under a 502.
- **Docs:** the cwd is decided at the start; an agent moving elsewhere afterwards is within the honest threat model (umbrella §8.4).

**Spec amendments:**
- decision 2: ACP core §9: a start on a host that is away answers 409 `host_offline` and creates no session.
- decisions 3 and 4: ACP core §4.3: `invalid_cwd`, `cwd_moved` and `hat_ambiguous` refuse a start or a resume.
- decision 1: ACP core §8: `sessions.hat_rule_id`.
- decision 5: ACP core §3.3: `start_session` and `resume_session` can be refused `cwd_not_canonical`.
- decision 6: ACP core §9: the session list's `hat` filter, and `hat_id` on the list item.

Then, in order:
- **(5d) Re-assignment**
- **(4) Frontend shell**
- **(8) Gateway**

---

_Generated with Claude AI — please review before distribution._
