# Hats (plan 5b): `resolve_path` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Paths are resolved where the filesystem is: on the host (kernel spec §5.2, §5.4; umbrella §8.2).
- The `resolve_path` request and its `resolved_path` reply are on the wire. The host canonicalises a typed path: absolute or `~/…`, symlinks resolved, no `.` or `..`, no trailing slash.
- It is a probe of its own capability (`resolve_path`), through the probe map plan 6c built: answered only by the connection it went out on, sent only to a host that announced the capability.
- `POST /api/hats/resolve` reports the canonical path and the hat it resolves to.
- `PUT /api/hosts/{id}/path-rules` resolves every prefix through the host, so rules are saved only with the host connected.

No session carries a hat yet: that is plan 5c. Re-assignment is plan 5d.

**Architecture:**
- **Host** (`hennery-host`):
  - `paths.rs` (new): `resolve(typed, home)`, pure and tested;
  - `projects.rs`: `Probes::resolve` answers `resolve_path` on a blocking thread, at most `MAX_RESOLVES` (4) at once and `busy` beyond, a panic answered too;
  - `connection.rs`: announces `Capability::ResolvePath`, and routes the frame to `Probes::resolve`.
- **Wire** (`hennery-proto`):
  - `CollectorFrame::ResolvePath`, `HostFrame::ResolvedPath`, `Capability::ResolvePath`;
  - their places in `probe_capability` and `probe_request_id`;
  - `HatResolveRequest`, `HatResolution`.
- **Sessions** (`hennery-sessions`):
  - `ws.rs`: `ResolvedPath` joins the probe arm;
  - `resolve.rs` (new): `resolve_on_host`, which checks the answer's form and maps the probe's outcomes;
  - `hats.rs`: `POST /api/hats/resolve`, and the rules resolved through the host.
- **Tests:**
  - the host's resolver: real symlinks, `~`, a missing tail, a `..` past it, a dangling symlink, a permission-denied ancestor, a file used as a directory, case on macOS, and a missing path's canonical form compared with the real one once it exists;
  - over HTTP, against a scripted host (wrong answers on purpose, no capability) and a real host (real symlinks).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md), the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md) and [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md), these sections:
- kernel §5.2: "Canonicalisation happens **on the host**, which is where the filesystem is (`resolve_path` request, §5.4). Rule prefixes are stored canonicalised the same way (resolved through the host when the rule is saved; a rule for a path that does not exist is stored as typed, normalised lexically, and marked unverified)."
- kernel §5.4: "`resolve_path{path}` → `resolved_path{canonical, exists, is_dir}` (or `error`) is part of the frame catalogue (ACP core §3.3). It is used for typed paths in New session, for session start and resume, for rule saving, and by the hat tester."
- kernel §8: `POST /api/hats/resolve` takes `{host_id, path}` and answers `{canonical, hat_id, rule_id?}`.
- umbrella §8.2: paths are "canonicalised **on the host** (absolute, symlinks resolved, `.`/`..` removed, no trailing slash) before matching".
- ACP core §3.3, the frame catalogue and capabilities; §3.4 as plan 6c amended it: a probe that times out keeps its connection.

It builds on the executed [plan 5a](2026-10-06-hats.md) and on [plan 6c](2026-10-07-projects.md)'s probe map. Read 5a's "After this plan" first: the 5b obligations there are this plan's scope. Read 6c's "What hats must do": this plan follows it. Every anchor below was taken from `main` at `d8d70e0`. Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** executed 2026-10-02 (see "Execution status"); amended after the security review.

The security review of 2026-10-02, binding on the maintainer's behalf, covered plans 5b, 5c and 5d together. It approved after amendments: B1–B3 required, P1 and P4–P7 taken, P2 and P3 recorded (see "Decisions", "What the review changed"). It then re-confirmed the amended code: "confirmed with notes".

**How the code blocks were made and checked:**
- Every block below was generated from the code as executed and then rebased onto plan 6c's probe map (see "Execution status").
- The plan was replayed from its own text onto `d8d70e0`. After each task's Step 1 the tree matched the tests-only commit, and after each task the task commit, byte for byte, the generated files included.
- After every task the five checks of "Global Constraints" passed: 794 tests after Task 1, 797 after Task 2, 804 after Task 3 (from 783 on `d8d70e0`).
- The guards were revert-probed (each task's "Revert-probes" step).

## Execution status (2026-10-02)

**Executed** with subagent-driven development, on `main` at `58624d7`. Each task had one implementer (sonnet), then an opus review. Then came an opus whole-branch review, then a rebase onto plan 6c's probe map, reviewed on its own (opus).

| Area | As built | Why |
|---|---|---|
| Task 1, fix round 1 (review, 2 Important) | A `..` after the first missing component is refused. Only `NotFound` takes the missing-path route: a permission-denied ancestor, a file used as a directory, a name too long, and a loop are all refused. New tests: those inputs, plus a property check that a missing path's canonical form equals the filesystem's once the directories exist. | Before the fix, `missing/../link/x` came back as `…/link/x` (the real one: `…/elsewhere/x`), and `missing/../dangling/x` slipped past P7. One directory had two "canonical" strings, so a rule and a session could disagree about a hat. The plan's own code had this defect, and the plan text below is the fixed code. |
| Task 3, fix round 1 (review, Important) | The rule limit (256) and the hat check run before any probe. The route resolves 4 prefixes at once, the host's limit. | A 10,000-rule body sent 10,000 probes before the kernel refused it. |
| The final review (opus) | A prefix that exists but is not a directory is refused (400). The host answers a panicking resolution instead of leaving it to the timeout. `resolve_path` is a capability (`Capability::ResolvePath`), so a host built before this plan is never sent one: 409 `resolve_unsupported`. | Without the capability, an older host ignored the frame and the collector waited it out. The not-a-directory prefix could never cover a session. |
| Rebase onto plan 6c (`b2aee2e`, then `d8d70e0`) | 5b's own probe map is gone. `resolve_path` is a probe of the map 6c built (`Hub::probe`, `probe_reply`, `probe_capability`, `probe_request_id`), answered by the host's `Probes` executor (`try_acquire`: `busy` when full). A probe timeout keeps the connection (6c's decision 1, its own review). `resolve_on_host` maps `Unsupported` to 409 `resolve_unsupported`, and `Busy` (the hub's, or the host's `busy` code) to 503 `busy`. It uses the state's `probe_timeout`, 6c's 15 s.

The rebase's own review (opus) found three Important issues, fixed in one commit:
- a host's panic now answers `internal` (502), not `invalid` (400);
- a probe that gets no answer is 503 `no_answer`, as in 6c, since the connection is kept;
- the tests count probes with `pending_probes`, since `pending_requests` now counts only session waiters.

It also took three Minor ones:
- the host releases a resolution's slot before it replies;
- the host's `busy` is pinned by a test;
- host text is shown only when `is_displayable_text` holds, as 6c's A6 asks. | Both lanes agreed the names; whoever merged first owned the map, and 6c did. 5b's hub tests went with its map; 6c's cover the map. The ruling "a timeout kicks the connection" is superseded by 6c's reviewed decision, since a probe changes no state. The host check of 5b's map is moot: 6c scopes replies by `conn_id`, and connection ids are unique across hosts. |

Process notes:
- Two implementers ran `git stash` against the fleet rule. Each recovered, and its tree was verified against a fresh replay. A hook is proposed to the operator.
- A shared `CARGO_TARGET_DIR` gave one false `gen --check`; each worktree now has its own.
- A CLI test (`the_collector_refuses_a_listen_fd_that_is_not_a_listening_socket`) failed twice under machine load and passed alone. It is `main`'s own test, now handed to the debt lane.

Deferred minors, triaged by the final review as able to wait:
- A refusal still waits for every other probe of a rule set.
- An unknown host gets 409 from the resolver but 404 from `PUT`.
- "In the order given" is not pinned by a test.
- The host-exists check reads every rule.
- `resolved_path` and the `ws.rs` routing order have no unit test of their own (6c's probe tests cover the map).

Tests: 804 in the workspace, on `d8d70e0`.

## Scope

5a hands 5b these items ("After this plan"):
- the typed prefix goes through the host's `resolve_path` first, where `~`, `..` and symlinks are resolved;
- `verified` comes only from the host's answer;
- the host refuses a canonical form that is not UTF-8, and the collector accepts an answer only in canonical form, at most 4096 bytes long;
- a missing path's deepest existing ancestor is resolved;
- two typed prefixes that resolve to one are refused as a set.

6c hands it: go through the probe map unchanged, classified in both exhaustive matches.

That is **3 tasks**:
1. the host's path resolver;
2. `resolve_path` on the wire, a probe of its own capability;
3. `POST /api/hats/resolve`, and rules resolved through their host.

**Out** (see "After this plan"):
- sessions carrying a hat (5c), and re-assignment (5d);
- re-verifying rules on connect;
- 6c's suggested refactors (one home for the path constants, a probe-route helper, a `ProbeError` type).

## Decisions this plan makes where the spec is silent

A stronger-model security review, made on the maintainer's behalf on 2026-10-02, confirmed these: "approve after amendments", then "confirmed with notes". Items marked **(amendment)** depart from explicit spec text and should be written back into it.

**What the review changed (for 5b):**
- B1: rules are saved only with their host connected. Any non-empty set for a host that is away is 409 `host_offline`; the empty set clears them. By its text alone, a prefix under a symlinked parent misses every session under it, silently: `/tmp` and `/var` on macOS, `/home` on Fedora Atomic, a home on another volume.
- B3: the spec write-back (kernel §5.2, §5.4, §8; ACP core §3.3), done in this plan's record commit.
- Optional hardening taken:
  - P5: a host error code other than `invalid` (or the probes' `busy`) is answered 502 `host_refused`, so a host cannot make the collector say `step_up_required`.
  - P6: the host resolves at most four paths at once.
  - P7: a dangling symlink in a missing path's tail is refused.

1. **The frames.**
   - `CollectorFrame::ResolvePath{request_id, path}` and `HostFrame::ResolvedPath{request_id, canonical, exists, is_dir}`, not outboxed.
   - A refusal is `HostFrame::Error{request_id, code: "invalid", message}`; a host with its slots full answers `busy`.
2. **A probe of its own capability.**
   - `Capability::ResolvePath`, which hosts from this plan on announce. `probe_capability` maps the frame to it, so a host that has not announced it is never sent one (ACP core §3.3): 409 `resolve_unsupported`.
   - `ResolvedPath` is a probe reply in `probe_request_id` and in `ws.rs`'s probe arm. Only the connection the probe went out on can answer it.
   - The probe's timeout is 6c's `PROBE_TIMEOUT`, and it keeps the connection (6c's decision 1).
3. **The host's resolution** (`paths::resolve`). (amended after the security review of 2026-10-02: P6, P7, and Task 1's fix round)
   - `~` and `~/…` are the host user's home. `~user`, a relative path, a control character, more than 4096 bytes, and a canonical form that is not UTF-8 are refused. Nothing is converted lossily.
   - An existing path goes through `std::fs::canonicalize`; on macOS that gives the case on disk.
   - A missing path keeps its deepest existing ancestor canonicalised, and the rest is appended by its text (amendment of "stored as typed"). These are refused:
     - a `..` after the part that exists;
     - a dangling symlink, or a loop, as the first missing component (P7);
     - a file used as a directory;
     - any error other than "not found", such as permission denied.
   - It runs in the host's probe executor: at most four at once, `busy` beyond (P6). The slot is released before the reply goes out, and a panic is answered `internal`.
4. **The collector checks every answer.**
   - `canonical` is accepted only in canonical form (`is_canonical`); otherwise 502 `bad_host_answer`.
   - A refusal coded `invalid` is 400 `invalid`, with the host's message only if it is displayable (6c's A6), and `busy` is 503. Any other code, a host's `internal` panic included, is 502 `host_refused`, with a message of the collector's own (P5).
   - An offline host is 409 `host_offline`. No answer in time, or a lost connection, is 503 `no_answer`.
5. **Rules are resolved through their host, or not saved.** (B1) (amendment)
   - `PUT /api/hosts/{id}/path-rules` checks, before any probe, that the host exists (404), that there are at most 256 rules, and that every hat is the owner's (400).
   - Then every typed prefix goes to the host, at most four at once, in order. `verified` is the host's `exists`. A prefix that exists but is not a directory is refused (400). The first refusal is the answer, and nothing is stored.
   - Two prefixes that resolve to one are refused as a set.
6. **`POST /api/hats/resolve`** takes `{host_id, path}` and answers `{canonical, exists, is_dir, hat_id, rule_id?}`. It needs the operator but no step-up. In 5c it starts refusing `hat_ambiguous` where a start would (P1).
7. **What a host can do.** It is trusted only for its own filesystem. It can lie about which directory a path is, and so pick any hat among its default and its rules' hats. That is within umbrella §8.4; recorded for the gateway.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **No new crates;** `Cargo.lock` does not change.
- **Wire types change in Tasks 2 and 3.** The generated files are regenerated there with `cargo run -p hennery-proto --bin gen`; new root types go in both `codegen.rs` lists.
- Every new route is an operator's (`operator_only`), and takes its body as `ApiJson`.
- **No Linux-only code**, and no test that reads another process's state without polling for a positive signal (fleet rule). CI's `ubuntu-latest` is the only Linux check.
- Use a `CARGO_TARGET_DIR` of the worktree's own. Never run `git stash`.
- Commits: Conventional Commits, gmail identity, unsigned. Push the feature branch after every task; never push `main`.

## Review Focus

1. **Paths under a symlink, as macOS gives them** (`/tmp`, `/var/folders/…`), for rules and the hat tester.
   - Expected: the canonical form, the rule verified when it exists, and the hat its rules give.
   - Tests: Task 1's resolver tests; Task 3 `a_real_host_resolves_symlinks_for_the_resolver_and_for_rules`.
2. **A missing path that climbs back, or is blocked.** Inputs: `missing/../link/x`, a dangling link, a directory with no permission, a file used as a directory.
   - Expected: refused with a reason, never a string that is not the filesystem's.
   - Tests: Task 1, `a_missing_paths_canonical_form_matches_the_real_one_once_it_exists` among them.
3. **A host that answers wrongly, refuses oddly, goes away, or is too old.**
   - Expected: 502 `bad_host_answer`; 400 `invalid` or 502 `host_refused`; 503 `no_answer` or `busy`; 409 `resolve_unsupported`. No probe left behind.
   - Tests: Task 3 `an_answer_not_in_canonical_form_is_refused`, `a_refusal_an_offline_host_and_a_lost_connection_each_answer_plainly`, and the no-capability test.
4. **Saving rules while the host is away, too many rules, or rules that collapse into one.**
   - Expected: 409 `host_offline`; 400 before any probe; 400 for two prefixes resolving to one. Nothing stored.
   - Tests: Task 3 `path_rules_are_saved_only_with_their_host_connected`, `a_connected_host_resolves_and_verifies_rule_prefixes`.
5. **Typed paths that are not quite paths:** `~`, `~other`, a relative path, control characters, an over-long path.
   - Expected: `~` is the host's home; the rest is refused.
   - Tests: Task 1 `a_tilde_is_the_hosts_home_and_nothing_else_is_relative`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-host/src/paths.rs` (new), `lib.rs` | The resolver | 1 |
| `crates/hennery-proto/src/frames.rs`, generated files | The frames, the capability, their probe mappings | 2 |
| `crates/hennery-host/src/projects.rs`, `connection.rs` | `Probes::resolve`; the capability announced; the route | 2 |
| `crates/hennery-sessions/src/ws.rs` | `ResolvedPath` in the probe arm | 2 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs` | The resolve types | 3 |
| `crates/hennery-sessions/src/resolve.rs` (new), `lib.rs`, `hats.rs` | `resolve_on_host`, the routes | 3 |
| Tests: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/host_connection.rs` (the announced capabilities), `crates/hennery-testkit/tests/resolve.rs` (new), `crates/hennery-testkit/tests/hats.rs` (rules need their host) | | 2, 3 |

All commands run from the repository root inside the dev shell. Work on a feature branch off `main`. **Reading the steps:** as in plan 5a. "Create `path`:", "Replace the whole of `path` with:", and "In `path`, replace:" are each followed by a block that occurs exactly once at that point, then "with:". "Run: `cargo run -p hennery-proto --bin gen`" regenerates the generated files.

---

### Task 1: The host's path resolver

**Files:**
- Create: `crates/hennery-host/src/paths.rs` (with its unit tests)
- Modify: `crates/hennery-host/src/lib.rs`

**Interfaces:**
- Produces: `hennery_host::paths::{resolve(typed: &str, home: Option<&Path>) -> Result<Resolved, String>, Resolved {canonical, exists, is_dir}, MAX_PATH}`.
- Consumes: nothing.

- [ ] **Step 1: Write the failing tests**

None apart: the resolver is a new module, and its unit tests come with it in Step 3.

- [ ] **Step 2: Run them to see them fail**

Nothing to run.

- [ ] **Step 3: The resolver**

In `crates/hennery-host/src/lib.rs`, replace:

```rust
pub mod pairing;
```

with:

```rust
pub mod pairing;
pub mod paths;
```

Create `crates/hennery-host/src/paths.rs`:

```rust
//! Resolving a typed path on the host (kernel spec §5.2, §5.4), where the
//! filesystem is: what a session's hat is decided on. The canonical form
//! is absolute, symlinks resolved, with no `.`, `..` or trailing slash
//! (umbrella §8.2), the same form the agents themselves key projects by.

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// The longest path accepted, in bytes: Linux's `PATH_MAX`, as the
/// collector's rule prefixes (`hennery_kernel::hats::MAX_PATH`).
pub const MAX_PATH: usize = 4096;

/// A resolved path (`resolved_path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub canonical: String,
    pub exists: bool,
    pub is_dir: bool,
}

/// Resolve `typed`: `~` and `~/…` are the host user's `home`, anything
/// else must be absolute. A path that exists is canonicalised by the
/// filesystem. One that does not has its deepest existing ancestor
/// canonicalised and the rest normalised by its text: a rule saved for a
/// directory not made yet then still matches it once it exists under a
/// symlinked parent. Refuses a relative path, `~user`, control characters,
/// a path longer than `MAX_PATH`, and a canonical form that is not UTF-8.
pub fn resolve(typed: &str, home: Option<&Path>) -> Result<Resolved, String> {
    if typed.len() > MAX_PATH {
        return Err(format!("a path must be at most {MAX_PATH} bytes"));
    }
    if typed.chars().any(char::is_control) {
        return Err("a path must not hold control characters".into());
    }
    let path = expand_home(typed, home)?;
    if !path.is_absolute() {
        return Err("a path must be absolute, or start with ~/".into());
    }
    let (canonical, exists) = match std::fs::canonicalize(&path) {
        Ok(canonical) => (canonical, true),
        Err(e) if e.kind() == ErrorKind::NotFound => (resolve_missing(&path)?, false),
        Err(e) => return Err(e.to_string()),
    };
    let is_dir = exists && canonical.is_dir();
    let canonical = canonical
        .into_os_string()
        .into_string()
        .map_err(|_| "the resolved path is not UTF-8".to_string())?;
    if canonical.len() > MAX_PATH {
        return Err(format!("the resolved path is longer than {MAX_PATH} bytes"));
    }
    Ok(Resolved {
        canonical,
        exists,
        is_dir,
    })
}

fn expand_home(typed: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    let rest = match typed.strip_prefix('~') {
        None => return Ok(PathBuf::from(typed)),
        Some("") => "",
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/'),
        Some(_) => return Err("only ~ and ~/… name a home directory here".into()),
    };
    let home = home.ok_or("this host has no home directory to expand ~ to")?;
    Ok(home.join(rest))
}

/// `path` (absolute, not there) with its deepest existing ancestor
/// canonicalised and the components after it applied by their text. The
/// first of those must not be a symlink that does not resolve (the
/// review's P7): its name would stand in the result, and once the target
/// exists, paths under it would resolve to the target instead. Nothing
/// after that first missing component can exist, so a `..` there is
/// refused rather than climbed, which would otherwise land back in a
/// part of the path that does exist and so needs canonicalising, not
/// kept by its text. Any canonicalize error besides "not found", on the
/// ancestor or on the first missing component's own entry, is refused
/// with its own message rather than treated as missing (a permission
/// error must not be read as "nothing here").
fn resolve_missing(path: &Path) -> Result<PathBuf, String> {
    let components: Vec<Component<'_>> = path.components().collect();
    for split in (1..components.len()).rev() {
        let ancestor: PathBuf = components[..split].iter().collect();
        let mut out = match std::fs::canonicalize(&ancestor) {
            Ok(out) => out,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        if !out.is_dir() {
            return Err(format!("{:?} is not a directory", out.display()));
        }
        if let Component::Normal(name) = components[split] {
            match out.join(name).symlink_metadata() {
                Ok(_) => {
                    return Err(format!(
                        "{:?} is a symlink that does not resolve",
                        out.join(name).display()
                    ));
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        for component in &components[split..] {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err("a path must not climb out of a part that does not exist with `..`".into());
                }
                Component::Normal(name) => out.push(name),
                Component::RootDir | Component::Prefix(_) => return Err("not a plain absolute path".into()),
            }
        }
        return Ok(out);
    }
    Err("no part of the path exists on this host".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn canonical(p: &Path) -> String {
        std::fs::canonicalize(p)
            .unwrap()
            .into_os_string()
            .into_string()
            .unwrap()
    }

    #[test]
    fn an_existing_path_is_canonicalised_through_its_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(real.join("x")).unwrap();
        symlink(&real, dir.path().join("link")).unwrap();
        let root = canonical(dir.path());
        for typed in ["link/x", "link/x/", "link/./x", "real/x/../x", "link//x"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert_eq!(
                resolve(&typed, None),
                Ok(Resolved {
                    canonical: format!("{root}/real/x"),
                    exists: true,
                    is_dir: true
                }),
                "{typed}"
            );
        }
        std::fs::write(real.join("file"), b"").unwrap();
        let file = resolve(&format!("{}/link/file", dir.path().display()), None).unwrap();
        assert_eq!(
            (file.canonical, file.exists, file.is_dir),
            (format!("{root}/real/file"), true, false)
        );
    }

    /// Kernel spec §5.2's unverified rule, resolved as far as it exists: a
    /// directory not made yet under a symlinked parent gets the parent's
    /// canonical form.
    #[test]
    fn a_missing_path_keeps_its_existing_ancestor_resolved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let root = canonical(dir.path());
        let typed = format!("{}/link/not/yet/made/", dir.path().display());
        assert_eq!(
            resolve(&typed, None),
            Ok(Resolved {
                canonical: format!("{root}/real/not/yet/made"),
                exists: false,
                is_dir: false
            })
        );
    }

    #[test]
    fn a_dangling_symlink_is_refused_not_kept_by_its_name() {
        let dir = tempfile::tempdir().unwrap();
        symlink(dir.path().join("not-yet"), dir.path().join("link")).unwrap();
        for typed in ["link", "link/x"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert!(resolve(&typed, None).is_err(), "{typed}");
        }
    }

    /// Review finding (Important 1): a `..` after the first missing
    /// component must not climb back into a part of the path that does
    /// exist (`elsewhere`, reached only through the `link` symlink, or a
    /// dangling symlink that P7 would otherwise catch) and be kept by its
    /// text instead of canonicalised, or bypassed entirely.
    #[test]
    fn a_dotdot_after_a_missing_component_is_refused_not_climbed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, dir.path().join("link")).unwrap();
        symlink(dir.path().join("not-yet"), dir.path().join("dangling")).unwrap();
        for typed in ["missing/../link/x", "missing/../dangling/x", "missing/../link"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert!(resolve(&typed, None).is_err(), "{typed}");
        }
    }

    /// Review finding (Important 2): a file cannot hold a path under it.
    /// Through `resolve`, the top-level canonicalize of the whole path
    /// fails with `NotADirectory`, not `NotFound`, so it is refused there
    /// first, before `resolve_missing`'s own `is_dir` guard ever runs —
    /// see the direct `resolve_missing` test below for that guard in
    /// isolation.
    #[test]
    fn a_file_used_as_a_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), b"").unwrap();
        let typed = format!("{}/file/x", dir.path().display());
        assert!(resolve(&typed, None).is_err());
    }

    /// Review finding (Important 2), isolated: called directly (bypassing
    /// `resolve`'s own top-level `NotADirectory` refusal above),
    /// `resolve_missing`'s `is_dir` guard is what refuses a file used as
    /// a directory, with its own message.
    #[test]
    fn resolve_missing_refuses_a_file_used_as_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file"), b"").unwrap();
        let err = resolve_missing(&dir.path().join("file/x")).unwrap_err();
        assert!(err.contains("is not a directory"), "{err}");
    }

    /// Review finding (Important 2, minor): a symlink loop as the first
    /// missing component is refused, not misreported as a missing target.
    #[test]
    fn a_symlink_loop_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        symlink(dir.path().join("b"), dir.path().join("a")).unwrap();
        symlink(dir.path().join("a"), dir.path().join("b")).unwrap();
        for typed in ["a", "a/x"] {
            let typed = format!("{}/{typed}", dir.path().display());
            assert!(resolve(&typed, None).is_err(), "{typed}");
        }
    }

    /// Review finding (Important 2): a permission error part-way down the
    /// path must be refused with its own message, not read as "missing".
    /// Skipped as root, which ignores directory permissions outright.
    #[test]
    fn a_permission_denied_ancestor_is_refused_not_reported_missing() {
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: geteuid(2) cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let typed = format!("{}/blocked/rest/of/path", dir.path().display());
        let result = resolve(&typed, None);
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err());
    }

    /// The unverified rule (kernel spec §5.2) actually holds: once the
    /// directories a missing resolve predicted are made, canonicalizing
    /// the same typed path for real lands on the same string.
    #[test]
    fn a_missing_paths_canonical_form_matches_the_real_one_once_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        for rest in ["a/b", "c"] {
            let typed = format!("{}/link/{rest}", dir.path().display());
            let resolved = resolve(&typed, None).unwrap();
            assert!(!resolved.exists, "{typed}");
            std::fs::create_dir_all(dir.path().join("real").join(rest)).unwrap();
            assert_eq!(resolved.canonical, canonical(Path::new(&typed)), "{typed}");
        }
    }

    #[test]
    fn a_tilde_is_the_hosts_home_and_nothing_else_is_relative() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("Projects")).unwrap();
        let root = canonical(home.path());
        assert_eq!(resolve("~", Some(home.path())).unwrap().canonical, root);
        assert_eq!(
            resolve("~/Projects", Some(home.path())).unwrap().canonical,
            format!("{root}/Projects")
        );
        assert_eq!(
            resolve("~//Projects/", Some(home.path())).unwrap().canonical,
            format!("{root}/Projects")
        );
        for bad in ["~other/Projects", "Projects", "./Projects", "", "/p/\0x", "/p/\nx"] {
            assert!(resolve(bad, Some(home.path())).is_err(), "{bad:?}");
        }
        assert!(resolve("~/Projects", None).is_err());
        assert!(resolve(&format!("/{}", "a".repeat(MAX_PATH)), None).is_err());
    }

    /// On a case-insensitive filesystem (macOS by default) the canonical
    /// form has the case on disk, so a rule and a session typed in other
    /// cases resolve alike (plan 5a decision 9).
    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_the_canonical_form_has_the_case_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Acme")).unwrap();
        if !dir.path().join("ACME").exists() {
            // A case-sensitive volume: there is no other case to fold.
            return;
        }
        let root = canonical(dir.path());
        let typed = format!("{}/acme", dir.path().display());
        assert_eq!(resolve(&typed, None).unwrap().canonical, format!("{root}/Acme"));
    }
}
```

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-host --locked --lib paths::`
Expected: PASS, 11 tests. The case test returns early on a case-sensitive volume, and the permission test returns early as root.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `resolve_missing`, remove the `..` refusal. `a_dotdot_after_a_missing_component_is_refused_not_climbed` fails.
- In `resolve_missing`, remove the dangling-symlink check. `a_dangling_symlink_is_refused_not_kept_by_its_name` fails.
- Take the missing-path route on any error, in all three places: the whole path in `resolve`, each ancestor in `resolve_missing`, and its first missing component. `a_permission_denied_ancestor_is_refused_not_reported_missing` fails. Each guard alone is backed by the other two, so removing one is caught by no test.
- In `expand_home`, accept anything after `~`. `a_tilde_is_the_hosts_home_and_nothing_else_is_relative` fails on `~other/Projects`.

- [ ] **Step 6: The full checks**

Run the five commands. Expected: all pass; **794 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-host
git commit -m "feat(host): resolve a typed path where the filesystem is"
```

### Task 2: `resolve_path` on the wire, a probe of its own capability

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`, `crates/hennery-host/src/projects.rs`, `crates/hennery-host/src/connection.rs`, `crates/hennery-sessions/src/ws.rs`, the generated files
- Test: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Produces:
  - `CollectorFrame::ResolvePath{request_id, path}`, `HostFrame::ResolvedPath{request_id, canonical, exists, is_dir}` and `Capability::ResolvePath`, with their probe mappings;
  - `Probes::resolve(uplink, request_id, path, home)` and `MAX_RESOLVES` (4).
- Consumes: Task 1's `paths::resolve`; 6c's probe map and `Probes`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            content: None,
```

with:

```rust
            content: None,
        },
        CollectorFrame::ResolvePath {
            request_id: "r".into(),
            path: "~/Projects".into(),
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
    assert_eq!(serde_json::to_value(&body).unwrap(), full);
}
```

with:

```rust
    assert_eq!(serde_json::to_value(&body).unwrap(), full);
}

/// Kernel spec §5.4: `resolve_path{path}` → `resolved_path{canonical,
/// exists, is_dir}`, each with its request id.
#[test]
fn resolve_path_and_its_answer_use_the_spec_field_names() {
    let request = CollectorFrame::ResolvePath {
        request_id: "r".into(),
        path: "~/p".into(),
    };
    let wire = json!({"type": "resolve_path", "request_id": "r", "path": "~/p"});
    assert_eq!(serde_json::to_value(&request).unwrap(), wire);
    assert_eq!(serde_json::from_value::<CollectorFrame>(wire).unwrap(), request);
    let answer = HostFrame::ResolvedPath {
        request_id: "r".into(),
        canonical: "/home/me/p".into(),
        exists: true,
        is_dir: false,
    };
    let wire = json!({
        "type": "resolved_path", "request_id": "r", "canonical": "/home/me/p", "exists": true, "is_dir": false
    });
    assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
    assert_eq!(serde_json::from_value::<HostFrame>(wire).unwrap(), answer);
}
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        Capabilities(vec![Capability::Park, Capability::Images, Capability::Projects])
```

with:

```rust
        Capabilities(vec![
            Capability::Park,
            Capability::Images,
            Capability::Projects,
            Capability::ResolvePath
        ])
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-proto --locked --test frames`
Expected: FAIL to compile: `no variant named ResolvePath found for enum CollectorFrame`, `no variant named ResolvedPath found for enum HostFrame`.

- [ ] **Step 3: The frames, the capability, and the host's answer**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            capabilities: Capabilities(vec![Capability::Park, Capability::Images, Capability::Projects]),
```

with:

```rust
            capabilities: Capabilities(vec![
                Capability::Park,
                Capability::Images,
                Capability::Projects,
                Capability::ResolvePath,
            ]),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        }
        CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
```

with:

```rust
        }
        CollectorFrame::ResolvePath { request_id, path } => probes.resolve(uplink, request_id, path, cfg.home.clone()),
        CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
```

In `crates/hennery-host/src/projects.rs`, replace:

```rust
pub const MAX_BROWSES: usize = 4;
```

with:

```rust
pub const MAX_BROWSES: usize = 4;
/// Path resolutions (`resolve_path`, plan 5b) a host runs at once.
pub const MAX_RESOLVES: usize = 4;
```

In `crates/hennery-host/src/projects.rs`, replace:

```rust
/// in the connection loop, at most `MAX_LISTS` enumerations and
/// `MAX_BROWSES` listings at once. One more is answered `busy`; the
/// collector says so. The reply goes out on whatever connection is up when
```

with:

```rust
/// in the connection loop, at most `MAX_LISTS` enumerations, `MAX_BROWSES`
/// listings and `MAX_RESOLVES` path resolutions (plan 5b) at once. One more
/// is answered `busy`; the collector says so. The reply goes out on whatever connection is up when
```

In `crates/hennery-host/src/projects.rs`, replace:

```rust
    browses: Arc<Semaphore>,
```

with:

```rust
    browses: Arc<Semaphore>,
    resolves: Arc<Semaphore>,
```

In `crates/hennery-host/src/projects.rs`, replace:

```rust
            browses: Arc::new(Semaphore::new(browses)),
```

with:

```rust
            browses: Arc::new(Semaphore::new(browses)),
            resolves: Arc::new(Semaphore::new(MAX_RESOLVES)),
```

In `crates/hennery-host/src/projects.rs`, replace:

```rust

fn refusal(request_id: String, refused: BrowseError) -> HostFrame {
```

with:

```rust

impl Probes {
    /// Answer `resolve_path` (kernel spec §5.4): `path` resolved where the
    /// filesystem is (`crate::paths::resolve`), `~` being `home`. A refusal
    /// is `invalid` with its reason; a panic is answered `internal` with a
    /// fixed message instead, not left to the collector's timeout (the
    /// review's Important 1): the operator's bad input and a bug in this
    /// host must not look alike.
    pub fn resolve(&self, uplink: &Uplink, request_id: String, path: String, home: Option<PathBuf>) {
        let Ok(permit) = self.resolves.clone().try_acquire_owned() else {
            uplink.reply(busy(request_id));
            return;
        };
        let uplink = uplink.clone();
        tokio::task::spawn_blocking(move || {
            let outcome = std::panic::catch_unwind(|| crate::paths::resolve(&path, home.as_deref()));
            // Released before the reply goes out, so a caller's next probe
            // never finds this slot still held (the review's Minor 1).
            drop(permit);
            uplink.reply(resolved_reply(request_id, outcome));
        });
    }
}

/// The reply for a `resolve_path`'s outcome: resolved, the function's own
/// refusal (`invalid`, its reason), or a panic caught instead of crashing
/// the blocking thread (`internal`, a fixed message: the caller's input is
/// never blamed for this host's bug).
fn resolved_reply(
    request_id: String,
    outcome: std::thread::Result<Result<crate::paths::Resolved, String>>,
) -> HostFrame {
    match outcome {
        Ok(Ok(resolved)) => HostFrame::ResolvedPath {
            request_id,
            canonical: resolved.canonical,
            exists: resolved.exists,
            is_dir: resolved.is_dir,
        },
        Ok(Err(message)) => HostFrame::Error {
            request_id,
            code: "invalid".into(),
            message,
        },
        Err(_) => HostFrame::Error {
            request_id,
            code: "internal".into(),
            message: "resolving the path failed on this host".into(),
        },
    }
}

fn refusal(request_id: String, refused: BrowseError) -> HostFrame {
```

In `crates/hennery-host/src/projects.rs`, replace:

```rust
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }
}
```

with:

```rust
        assert!(err.to_string().contains("UTF-8"), "{err}");
    }

    /// The review's Important 1: a panic resolving a path is answered
    /// `internal`, not `invalid` as the function's own refusal is.
    #[test]
    fn a_panic_resolving_is_answered_internal_not_invalid() {
        let outcome: std::thread::Result<Result<crate::paths::Resolved, String>> = Err(Box::new("boom"));
        match resolved_reply("r1".into(), outcome) {
            HostFrame::Error {
                request_id,
                code,
                message,
            } => {
                assert_eq!(request_id, "r1");
                assert_eq!(code, "internal");
                assert_eq!(message, "resolving the path failed on this host");
            }
            other => panic!("{other:?}"),
        }
    }

    /// The function's own refusal, unlike a panic, is answered `invalid`
    /// with its reason.
    #[test]
    fn the_functions_own_refusal_is_answered_invalid() {
        let outcome: std::thread::Result<Result<crate::paths::Resolved, String>> = Ok(Err("x".into()));
        match resolved_reply("r2".into(), outcome) {
            HostFrame::Error {
                request_id,
                code,
                message,
            } => {
                assert_eq!(request_id, "r2");
                assert_eq!(code, "invalid");
                assert_eq!(message, "x");
            }
            other => panic!("{other:?}"),
        }
    }
}
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    Park,
```

with:

```rust
    Park,
    /// Resolving typed paths (`resolve_path`, kernel spec §5.4).
    ResolvePath,
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    },
    /// Sent once per connection after the unacked outbox has been resent.
```

with:

```rust
    },
    /// The answer to `resolve_path` (kernel spec §5.4). Not outboxed: a
    /// probe changes nothing, so a lost answer costs only a retry.
    /// `canonical` is absolute, symlinks resolved, with no `.`, `..` or
    /// trailing slash; for a path that does not exist, its deepest existing
    /// ancestor is resolved and the rest normalised by its text.
    ResolvedPath {
        request_id: String,
        canonical: String,
        exists: bool,
        is_dir: bool,
    },
    /// Sent once per connection after the unacked outbox has been resent.
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
            Self::ListProjects { .. } | Self::BrowseDirectory { .. } => Ok(Some(Capability::Projects)),
```

with:

```rust
            Self::ListProjects { .. } | Self::BrowseDirectory { .. } => Ok(Some(Capability::Projects)),
            Self::ResolvePath { .. } => Ok(Some(Capability::ResolvePath)),
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
            Self::Projects { request_id, .. } | Self::Directory { request_id, .. } => Some(request_id),
```

with:

```rust
            Self::Projects { request_id, .. }
            | Self::Directory { request_id, .. }
            | Self::ResolvedPath { request_id, .. } => Some(request_id),
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    },
    /// Completed by `session_parked{operator}` (ACP core §4.8).
```

with:

```rust
    },
    /// Resolve a typed path on the host, where the filesystem is (kernel
    /// spec §5.2, §5.4): absolute, or `~` / `~/…` for the host user's home.
    /// Completed by `resolved_path` | `error{invalid}`.
    ResolvePath {
        request_id: String,
        path: String,
    },
    /// Completed by `session_parked{operator}` (ACP core §4.8).
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
            frame @ (HostFrame::Projects { .. } | HostFrame::Directory { .. }) => {
```

with:

```rust
            frame @ (HostFrame::Projects { .. } | HostFrame::Directory { .. } | HostFrame::ResolvedPath { .. }) => {
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-proto --locked --test frames && cargo test -p hennery-testkit --locked --test host_connection a_host_announces`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

- In `probe_capability`, map `ResolvePath` to `None`. The no-capability test of Task 3 then fails, since the frame goes to a host that never announced it. Restore it.

- [ ] **Step 6: The full checks**

Run the five commands. Expected: all pass; **797 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates schema web
git commit -m "feat(host): resolve_path on the wire, a probe of its own capability"
```

### Task 3: `POST /api/hats/resolve`, and rules resolved through their host

**Files:**
- Create: `crates/hennery-sessions/src/resolve.rs`
- Modify: `crates/hennery-sessions/src/lib.rs`, `crates/hennery-sessions/src/hats.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, the generated files
- Test: `crates/hennery-testkit/tests/resolve.rs` (new), `crates/hennery-testkit/tests/hats.rs`

**Interfaces:**
- Produces:
  - `crate::resolve::{OnHost {canonical, exists, is_dir}, NotResolved, resolve_on_host(state, host_id, path) -> Result<OnHost, NotResolved>}`;
  - `POST /api/hats/resolve` `HatResolveRequest` → `HatResolution` | 400 | 404 | 409 | 502 | 503;
  - `PUT /api/hosts/{id}/path-rules` through the host.
- Consumes: Task 2's frames; 6c's `Hub::probe`; 5a's `Hosts::{resolve_hat, replace_path_rules, path_rules, hat}` and `is_canonical`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/hats.rs`, replace:

```rust
//! replaces a host's path rules. Changing a host or its rules needs a
//! fresh step-up (plan 5a decisions 7 and 8; `step_up.rs` covers the
//! refusals).
```

with:

```rust
//! replaces a host's path rules, which needs the host connected (plan 5b).
//! Changing a host or its rules needs a fresh step-up (plan 5a decisions 7
//! and 8; `step_up.rs` covers the refusals).
```

In `crates/hennery-testkit/tests/hats.rs`, replace:

```rust
/// Kernel spec §5.2, §8: the full set replaces the host's rules; each
/// prefix is normalised by its text and stored unverified until the host
/// can resolve it (plan 5a decision 6).
#[tokio::test]
async fn path_rules_are_replaced_as_a_set_normalised_and_unverified() {
```

with:

```rust
/// Kernel spec §5.2: rules are resolved through their host when they are
/// saved, so with the host away a set with any rule is refused and stores
/// nothing (the review's B1); the empty set clears them. A connected host
/// resolving them is `resolve.rs`'s.
#[tokio::test]
async fn path_rules_are_saved_only_with_their_host_connected() {
```

In `crates/hennery-testkit/tests/hats.rs`, replace:

```rust
    let resp = c
        .put_rules("host-1", &[("/p/acme/", &acme.id), ("/p//acme/./secret", &acme.id)])
        .await;
    assert_eq!(resp.status(), 200);
    let stored: Vec<PathRuleItem> = resp.json().await.unwrap();
    let shown: Vec<(&str, bool)> = stored.iter().map(|r| (r.prefix.as_str(), r.verified)).collect();
    assert_eq!(shown, [("/p/acme/secret", false), ("/p/acme", false)]);
```

with:

```rust
    let stored = c
        .state
        .hosts
        .replace_path_rules(
            "host-1",
            &[hennery_kernel::hats::NewRule {
                prefix: "/p/acme".into(),
                hat_id: acme.id.clone(),
                verified: true,
            }],
        )
        .unwrap();
    assert!(matches!(stored, hennery_kernel::hats::RulesChange::Done(_)));
    assert_eq!(
        code_of(c.put_rules("host-1", &[("/p/acme", &acme.id)]).await).await,
        (409, "host_offline".into())
    );
```

In `crates/hennery-testkit/tests/hats.rs`, replace:

```rust
    assert_eq!(resp.json::<Vec<PathRuleItem>>().await.unwrap(), stored);
    let resolved = c.state.hosts.resolve_hat("host-1", "/p/acme/x").unwrap().unwrap();
    assert_eq!(resolved.hat_id, acme.id);

    // A bad set changes nothing.
    for bad in [
        vec![("p/acme", acme.id.as_str())],
        vec![("~/acme", acme.id.as_str())],
        vec![("/p/acme", "hat-nope")],
        vec![("/p/a", acme.id.as_str()), ("/p/a/", acme.id.as_str())],
        vec![("/p/a\u{0}b", acme.id.as_str())],
        vec![("/p/a\u{202E}b", acme.id.as_str())],
        vec![("/p/x/../a", acme.id.as_str())],
        vec![("/", acme.id.as_str())],
    ] {
        assert_eq!(
            code_of(c.put_rules("host-1", &bad).await).await,
            (400, "invalid".into()),
            "{bad:?}"
        );
    }
    assert_eq!(c.state.hosts.path_rules("host-1").unwrap().unwrap().len(), 2);
    assert_eq!(
        code_of(c.put_rules("host-nope", &[]).await).await,
```

with:

```rust
    let listed: Vec<PathRuleItem> = resp.json().await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|r| (r.prefix.as_str(), r.verified))
            .collect::<Vec<_>>(),
        [("/p/acme", true)]
    );
    assert_eq!(
        code_of(c.put_rules("host-nope", &[("/p", &acme.id)]).await).await,
```

In `crates/hennery-testkit/tests/hats.rs`, replace:

```rust
    // The empty set removes them all.
```

with:

```rust
    // The empty set needs no host: it removes them all.
```

Create `crates/hennery-testkit/tests/resolve.rs`:

```rust
//! Resolving paths through their host (kernel spec §5.2, §5.4; plan 5b):
//! `POST /api/hats/resolve` and the path rules a connected host resolves.
//! A scripted host plays the wire frame by frame, so its answers can be
//! wrong on purpose; a real host resolves real symlinks.

use futures::{SinkExt, StreamExt};
use hennery_host::HostConfig;
use hennery_host::identity::HostKey;
use hennery_kernel::hats::{HatChange, NewRule};
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame};
use hennery_proto::rest::{HatResolution, PathRuleItem};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const HOST: &str = "host-1";

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hosts = Hosts::open(&db).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        hosts.register(HOST, &enrollment, 0).unwrap();
        let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn hat(&self, name: &str) -> String {
        match self.state.hosts.create_hat(name, None, 1).unwrap() {
            HatChange::Done(hat) => hat.id,
            other => panic!("{other:?}"),
        }
    }

    async fn ready(&self) {
        wait_for("host ready", || async { self.state.hub.is_ready(HOST).then_some(()) }).await;
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing the host.
struct ScriptedHost {
    ws: Ws,
}

impl ScriptedHost {
    /// `hello` and `resend_complete`, then wait until the host is ready.
    /// Announces `resolve_path` (every test here resolves one).
    async fn connect(collector: &Collector) -> Self {
        Self::connect_with(collector, Capabilities(vec![Capability::ResolvePath])).await
    }

    /// `connect` announcing `capabilities`.
    async fn connect_with(collector: &Collector, capabilities: Capabilities) -> Self {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities,
            workspace_roots: vec![],
            attached_sessions: vec![],
        })
        .await;
        assert!(matches!(host.next().await, CollectorFrame::HelloAck { .. }));
        host.send(&HostFrame::ResendComplete).await;
        collector.ready().await;
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                    Some(Ok(_)) => {}
                    other => panic!("collector connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a collector frame within 10s")
    }

    /// The next `resolve_path`: its request id and path.
    async fn resolve_request(&mut self) -> (String, String) {
        match self.next().await {
            CollectorFrame::ResolvePath { request_id, path } => (request_id, path),
            other => panic!("expected resolve_path, got {other:?}"),
        }
    }

    /// Answer the next `resolve_path` with `canonical`, existing as a
    /// directory or not at all.
    async fn answer(&mut self, canonical: &str, exists: bool) -> String {
        let (request_id, path) = self.resolve_request().await;
        self.send(&HostFrame::ResolvedPath {
            request_id,
            canonical: canonical.into(),
            exists,
            is_dir: exists,
        })
        .await;
        path
    }
}

async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A request from the owner's client, bounded so a hang fails the test.
fn send(collector: &Collector, method: &str, path: &str, body: Value) -> tokio::task::JoinHandle<(u16, Value)> {
    let c = hennery_testkit::operator_client(&collector.state.operator);
    let url = collector.url(path);
    let method: reqwest::Method = method.parse().unwrap();
    tokio::spawn(async move {
        let resp = c
            .request(method, url)
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .unwrap();
        (resp.status().as_u16(), resp.json().await.unwrap_or(Value::Null))
    })
}

fn resolve(collector: &Collector, path: &str) -> tokio::task::JoinHandle<(u16, Value)> {
    send(
        collector,
        "POST",
        "/api/hats/resolve",
        json!({ "host_id": HOST, "path": path }),
    )
}

/// Kernel spec §5.4, §8: the path as the host resolved it, and the hat it
/// resolves to there, with the deciding rule.
#[tokio::test]
async fn a_path_resolves_through_its_host_to_a_hat() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    let stored = collector
        .state
        .hosts
        .replace_path_rules(
            HOST,
            &[NewRule {
                prefix: "/home/me/acme".into(),
                hat_id: acme.clone(),
                verified: true,
            }],
        )
        .unwrap();
    let rule_id = match stored {
        hennery_kernel::hats::RulesChange::Done(rules) => rules[0].id.clone(),
        other => panic!("{other:?}"),
    };
    let mut host = ScriptedHost::connect(&collector).await;

    let call = resolve(&collector, "~/acme/x");
    assert_eq!(host.answer("/home/me/acme/x", true).await, "~/acme/x");
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let got: HatResolution = serde_json::from_value(body).unwrap();
    assert_eq!(
        got,
        HatResolution {
            canonical: "/home/me/acme/x".into(),
            exists: true,
            is_dir: true,
            hat_id: acme,
            rule_id: Some(rule_id),
        }
    );

    // A sibling sharing the prefix's text is the host's default hat.
    let call = resolve(&collector, "/home/me/acme-infra");
    host.answer("/home/me/acme-infra", false).await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let got: HatResolution = serde_json::from_value(body).unwrap();
    let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!((got.exists, got.hat_id, got.rule_id), (false, default, None));
}

/// The host is authenticated, but its answers are its own words (plan 5b
/// decision 2): a path not in canonical form is matched against nothing.
#[tokio::test]
async fn an_answer_not_in_canonical_form_is_refused() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    for bad in [
        "/home/me/acme/",
        "/home/me/../acme",
        "home/me",
        "/home//me",
        "/a\u{0}b",
        "",
    ] {
        let call = resolve(&collector, "/home/me/acme");
        host.answer(bad, true).await;
        let (status, body) = call.await.unwrap();
        assert_eq!(
            (status, body["code"].as_str()),
            (502, Some("bad_host_answer")),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn a_refusal_an_offline_host_and_a_lost_connection_each_answer_plainly() {
    let collector = Collector::start().await;
    let (status, body) = resolve(&collector, "/p").await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));

    let mut host = ScriptedHost::connect(&collector).await;
    let call = resolve(&collector, "relative");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "invalid".into(),
        message: "a path must be absolute".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (400, Some("invalid"), Some("a path must be absolute"))
    );
    // A code the host has no business choosing is not passed on (the
    // review's P5): it could make a client prompt for a step-up. Nor is
    // its text: the collector's own message is shown instead (Minor 4).
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "step_up_required".into(),
        message: "no".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (502, Some("host_refused"), Some("the host refused the request"))
    );

    // An `invalid` refusal whose text cannot be shown (the review's Minor
    // 4) is answered a fixed message instead, not the host's bytes.
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "invalid".into(),
        message: "x".repeat(257),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (400, Some("invalid"), Some("the host refused the path"))
    );

    let call = resolve(&collector, "/p");
    host.resolve_request().await;
    drop(host);
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("no_answer")));
    assert_eq!(collector.state.hub.pending_probes(), 0);
}

/// The review's Minor 1: the host's own `busy` is a probe's rejection,
/// answered as the probes' own is (503 `busy`), not passed on verbatim.
#[tokio::test]
async fn a_hosts_own_busy_answers_503_busy() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "busy".into(),
        message: "the host is busy resolving another path".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("busy")));
    assert_eq!(collector.state.hub.pending_probes(), 0);
}

/// Final review I1 (ACP core §3.3): the collector never sends a frame
/// that needs a capability to a host that lacks it. A host connected
/// without `resolve_path` is refused plainly, for the resolver and for
/// rules alike, and nothing is sent its way.
#[tokio::test]
async fn a_host_without_resolve_path_is_refused_without_a_frame() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    let mut host = ScriptedHost::connect_with(&collector, Capabilities::default()).await;

    let call = resolve(&collector, "/p");
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("resolve_unsupported")));

    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p", "hat_id": acme }] }),
    );
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("resolve_unsupported")));
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // Neither refusal queued the host anything: a frame sent now is the
    // first thing it sees.
    assert!(collector.state.hub.notify(
        HOST,
        CollectorFrame::Ack {
            session_id: "s1".into(),
            ack_seq: 0,
        },
    ));
    assert!(matches!(host.next().await, CollectorFrame::Ack { .. }));
}

/// Kernel spec §5.2: a rule's prefix is resolved through its host when it
/// is saved, verified if it exists there (plan 5b decision 3). Two typed
/// prefixes that resolve to one are refused as a set.
#[tokio::test]
async fn a_connected_host_resolves_and_verifies_rule_prefixes() {
    let collector = Collector::start().await;
    let acme = collector.hat("Acme");
    let mut host = ScriptedHost::connect(&collector).await;
    let rules =
        |a: &str, b: &str| json!({ "rules": [{ "prefix": a, "hat_id": acme }, { "prefix": b, "hat_id": acme }] });

    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        rules("~/acme", "/tmp/x/../acme/new"),
    );
    // Resolved in the order given; several may be out at once.
    let mut answered = Vec::new();
    for _ in 0..2 {
        let (request_id, path) = host.resolve_request().await;
        let (canonical, exists) = if path == "~/acme" {
            ("/home/me/acme", true)
        } else {
            ("/private/tmp/acme/new", false)
        };
        answered.push(path);
        host.send(&HostFrame::ResolvedPath {
            request_id,
            canonical: canonical.into(),
            exists,
            is_dir: exists,
        })
        .await;
    }
    answered.sort();
    assert_eq!(answered, ["/tmp/x/../acme/new", "~/acme"]);
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 200, "{body}");
    let stored: Vec<PathRuleItem> = serde_json::from_value(body).unwrap();
    let shown: Vec<(&str, bool)> = stored.iter().map(|r| (r.prefix.as_str(), r.verified)).collect();
    assert_eq!(shown, [("/private/tmp/acme/new", false), ("/home/me/acme", true)]);

    // `/p/Acme` and `/p/acme` on a case-insensitive volume: one directory.
    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        rules("/p/Acme", "/p/acme"),
    );
    host.answer("/p/Acme", true).await;
    host.answer("/p/Acme", true).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hosts.path_rules(HOST).unwrap().unwrap().len(), 2);
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // A refused prefix stores nothing.
    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        rules("/p", "~other/p"),
    );
    for _ in 0..2 {
        let (request_id, path) = host.resolve_request().await;
        let frame = if path == "/p" {
            HostFrame::ResolvedPath {
                request_id,
                canonical: "/p".into(),
                exists: true,
                is_dir: true,
            }
        } else {
            HostFrame::Error {
                request_id,
                code: "invalid".into(),
                message: "only ~ and ~/… name a home directory here".into(),
            }
        };
        host.send(&frame).await;
    }
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hosts.path_rules(HOST).unwrap().unwrap().len(), 2);
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // A prefix that exists as a file, not a directory, is refused (final
    // review M3): a session's cwd can never be one.
    let call = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p/file", "hat_id": acme }] }),
    );
    let (request_id, path) = host.resolve_request().await;
    assert_eq!(path, "/p/file");
    host.send(&HostFrame::ResolvedPath {
        request_id,
        canonical: "/p/file".into(),
        exists: true,
        is_dir: false,
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body["code"].as_str(), body["message"].as_str()),
        (400, Some("invalid"), Some("/p/file is not a directory on that host"))
    );
    assert_eq!(collector.state.hosts.path_rules(HOST).unwrap().unwrap().len(), 2);
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // Over the limit, or naming a hat that is not the owner's: refused
    // before any prefix reaches the host (the review's Important 1).
    let over_limit: Vec<Value> = (0..257)
        .map(|i| json!({ "prefix": format!("/p/r{i}"), "hat_id": acme }))
        .collect();
    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": over_limit }),
    )
    .await
    .unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hub.pending_probes(), 0);

    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [{ "prefix": "/p/new", "hat_id": "hat-nope" }] }),
    )
    .await
    .unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    assert_eq!(collector.state.hub.pending_probes(), 0);

    // Nothing was queued for the host by either refusal: the next frame it
    // gets is a later resolve's.
    let call = resolve(&collector, "/p");
    let (request_id, path) = host.resolve_request().await;
    assert_eq!(path, "/p");
    host.send(&HostFrame::ResolvedPath {
        request_id,
        canonical: "/p".into(),
        exists: true,
        is_dir: true,
    })
    .await;
    let (status, _) = call.await.unwrap();
    assert_eq!(status, 200);
}

/// A real host resolves symlinks and `~` (kernel spec §5.2: canonical on
/// the host), for the resolver and for rules alike.
#[tokio::test]
async fn a_real_host_resolves_symlinks_for_the_resolver_and_for_rules() {
    let collector = Collector::start().await;
    let data = tempfile::tempdir().unwrap();
    let tree = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tree.path().join("real/acme/x")).unwrap();
    std::os::unix::fs::symlink(tree.path().join("real"), tree.path().join("link")).unwrap();
    let real = std::fs::canonicalize(tree.path().join("real")).unwrap();
    let real = real.to_str().unwrap();
    let cfg = HostConfig::new(
        format!("ws://{}/api/hosts/ws", collector.addr),
        HOST,
        host_key(),
        data.path().to_path_buf(),
    );
    let host = tokio::spawn(async move { hennery_host::run(cfg).await });
    collector.ready().await;
    let acme = collector.hat("Acme");
    let link = tree.path().join("link");
    let link = link.to_str().unwrap();

    let (status, body) = send(
        &collector,
        "PUT",
        &format!("/api/hosts/{HOST}/path-rules"),
        json!({ "rules": [
            { "prefix": format!("{link}/acme/"), "hat_id": acme },
            { "prefix": format!("{link}/later"), "hat_id": acme },
        ] }),
    )
    .await
    .unwrap();
    assert_eq!(status, 200, "{body}");
    let stored: Vec<PathRuleItem> = serde_json::from_value(body).unwrap();
    let shown: Vec<(String, bool)> = stored.into_iter().map(|r| (r.prefix, r.verified)).collect();
    assert_eq!(
        shown,
        [(format!("{real}/later"), false), (format!("{real}/acme"), true)]
    );

    let (status, body) = resolve(&collector, &format!("{link}/acme/./x")).await.unwrap();
    assert_eq!(status, 200, "{body}");
    let got: HatResolution = serde_json::from_value(body).unwrap();
    assert_eq!(
        (got.canonical, got.exists, got.is_dir, got.hat_id),
        (format!("{real}/acme/x"), true, true, acme)
    );
    let (status, body) = resolve(&collector, "not/absolute").await.unwrap();
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    host.abort();
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test resolve`
Expected: FAIL to compile: `unresolved import hennery_proto::rest::HatResolution`.

- [ ] **Step 3: The resolution and the routes**

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::PathRuleInput,
        rest::PathRulesRequest,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::PathRuleInput,
        rest::PathRulesRequest,
        rest::HatResolveRequest,
        rest::HatResolution,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::PathRulesRequest,
    );
```

with:

```rust
        rest::PathRulesRequest,
        rest::HatResolveRequest,
        rest::HatResolution,
    );
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    pub rules: Vec<PathRuleInput>,
}
```

with:

```rust
    pub rules: Vec<PathRuleInput>,
}

/// `POST /api/hats/resolve` (kernel spec §8): which hat `path` resolves to
/// on `host_id`, as a session started there would get. The host resolves
/// the path; `~` and `~/…` are its user's home.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HatResolveRequest {
    pub host_id: String,
    pub path: String,
}

/// 200 to `POST /api/hats/resolve`: the canonical path, whether it exists
/// and is a directory there, and its hat, with the rule that decided it
/// (absent: the host's default hat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HatResolution {
    pub canonical: String,
    pub exists: bool,
    pub is_dir: bool,
    pub hat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub rule_id: Option<String>,
}
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
//! Hat endpoints (kernel spec §5, §8): the hats, and each host's path
//! rules. They sit beside the host endpoints, in the collector's one
//! router; the model is the kernel's (`hennery_kernel::hats`).
```

with:

```rust
//! Hat endpoints (kernel spec §5, §8): the hats, each host's path rules,
//! and resolving a path to its hat. They sit beside the host endpoints, in
//! the collector's one router; the model is the kernel's
//! (`hennery_kernel::hats`), and paths are resolved by their host
//! (`resolve`).
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
use crate::api::{error, internal};
```

with:

```rust
use crate::api::{error, internal};
use crate::resolve::{NotResolved, resolve_on_host};
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
use axum::routing::{get, patch};
use axum::{Json, Router, middleware};
use hennery_kernel::hats::{HatChange, HatRecord, NewRule, PathRule, RulesChange, normalize_lexically};
use hennery_kernel::json::ApiJson;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{CreateHatRequest, HatItem, PathRuleItem, PathRulesRequest, UpdateHatRequest};
```

with:

```rust
use axum::routing::{get, patch, post};
use axum::{Json, Router, middleware};
use futures::stream::{self, StreamExt};
use hennery_kernel::hats::{HatChange, HatRecord, MAX_RULES, NewRule, PathRule, RulesChange};
use hennery_kernel::json::ApiJson;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    CreateHatRequest, HatItem, HatResolution, HatResolveRequest, PathRuleInput, PathRuleItem, PathRulesRequest,
    UpdateHatRequest,
};

/// Rule prefixes resolved through the host at once, at most: matches the
/// host's own resolution semaphore (the review's P6).
const RESOLVING_AT_ONCE: usize = 4;
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
        )
```

with:

```rust
        )
        .route("/api/hats/resolve", post(resolve_hat))
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
/// §8, the full set), 200 with the stored set. Each prefix is normalised by
/// its text and stored unverified (plan 5a decision 6); resolving it
/// through the host comes with `resolve_path`.
```

with:

```rust
/// §8, the full set), 200 with the stored set. Each prefix is resolved
/// through the host (kernel spec §5.2), and verified if it exists there; a
/// prefix that does not exist keeps its deepest existing ancestor resolved
/// (plan 5b decision 3). With the host away, a set with any rule is refused
/// (409 `host_offline`, the review's B1): by its text alone a prefix under
/// a symlinked parent would miss every session under it, silently. The set's
/// size and each rule's `hat_id` are checked before anything reaches the
/// host (the review's Important 1): a set that will be refused anyway must
/// not cost the host a round trip per prefix.
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
    let mut rules = Vec::with_capacity(req.rules.len());
    for rule in req.rules {
        match normalize_lexically(&rule.prefix) {
            Ok(prefix) => rules.push(NewRule {
                prefix,
                hat_id: rule.hat_id,
                verified: false,
            }),
            Err(why) => return error(StatusCode::BAD_REQUEST, "invalid", format!("{:?}: {why}", rule.prefix)),
        }
    }
```

with:

```rust
    match state.hosts.path_rules(&host_id) {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => return internal(err),
    }
    if req.rules.len() > MAX_RULES {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid",
            format!("a host has at most {MAX_RULES} path rules"),
        );
    }
    for rule in &req.rules {
        match state.hosts.hat(&rule.hat_id) {
            Ok(Some(_)) => {}
            Ok(None) => return error(StatusCode::BAD_REQUEST, "invalid", format!("no hat {:?}", rule.hat_id)),
            Err(err) => return internal(err),
        }
    }
    let rules = match rules_through_host(&state, &host_id, req.rules).await {
        Ok(rules) => rules,
        Err(why) => return why.into_response(),
    };
```

In `crates/hennery-sessions/src/hats.rs`, replace:

```rust
        Ok(RulesChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
```

with:

```rust
        Ok(RulesChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

/// Every rule's prefix resolved through its connected host, a few at once,
/// in the order given. The first refusal is the answer, and nothing is
/// stored.
async fn rules_through_host(
    state: &AppState,
    host_id: &str,
    rules: Vec<PathRuleInput>,
) -> Result<Vec<NewRule>, NotResolved> {
    let resolved: Vec<_> = stream::iter(rules)
        .map(|rule| async move {
            let on_host = resolve_on_host(state, host_id, &rule.prefix).await;
            (rule, on_host)
        })
        .buffered(RESOLVING_AT_ONCE)
        .collect()
        .await;
    let mut out = Vec::with_capacity(resolved.len());
    for (rule, on_host) in resolved {
        let on_host = on_host?;
        // A rule's prefix names a directory (kernel spec §5.2): a file
        // that exists there can never hold a session's cwd.
        if on_host.exists && !on_host.is_dir {
            return Err(NotResolved::Refused {
                code: "invalid".into(),
                message: format!("{} is not a directory on that host", on_host.canonical),
            });
        }
        out.push(NewRule {
            prefix: on_host.canonical,
            hat_id: rule.hat_id,
            verified: on_host.exists,
        });
    }
    Ok(out)
}

/// `POST /api/hats/resolve` (kernel spec §8): the path resolved by its host,
/// and the hat it resolves to there.
async fn resolve_hat(State(state): State<AppState>, ApiJson(req): ApiJson<HatResolveRequest>) -> Response {
    let on_host = match resolve_on_host(&state, &req.host_id, &req.path).await {
        Ok(on_host) => on_host,
        Err(why) => return why.into_response(),
    };
    match state.hosts.resolve_hat(&req.host_id, &on_host.canonical) {
        Ok(Some(resolution)) => Json(HatResolution {
            canonical: on_host.canonical,
            exists: on_host.exists,
            is_dir: on_host.is_dir,
            hat_id: resolution.hat_id,
            rule_id: resolution.rule_id,
        })
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
pub mod projects;
```

with:

```rust
pub mod projects;
mod resolve;
```

Create `crates/hennery-sessions/src/resolve.rs`:

```rust
//! Resolving a typed path through its host (kernel spec §5.2, §5.4): the
//! host canonicalises, where the filesystem is, and the collector checks
//! the answer's form before anything is matched against it.

use crate::AppState;
use crate::api::error;
use crate::hub::RequestError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hennery_kernel::hats::is_canonical;
use hennery_kernel::hosts::is_displayable_text;
use hennery_proto::frames::{CollectorFrame, HostFrame};

/// A path as its host resolved it, its form checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OnHost {
    pub canonical: String,
    pub exists: bool,
    pub is_dir: bool,
}

/// Why a path did not resolve, and the answer each gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotResolved {
    /// 409 `host_offline`: not connected, or not reconciled.
    HostOffline,
    /// 400 `invalid` with the host's message: it refused the path. `busy`
    /// is the probes' own (503). Any other code the host chose is answered
    /// `host_refused` (the review's P5): a host must not make the
    /// collector say, for instance, `step_up_required`.
    Refused { code: String, message: String },
    /// 503 `busy`: the connection has its most probes in flight, or the
    /// host its most resolutions; nothing was resolved.
    Busy,
    /// 503 `no_answer`: the probe timed out, keeping the connection, or it
    /// dropped before answering; either way, no answer came.
    NoAnswer,
    /// 502 `bad_host_answer`: an answer not in canonical form.
    BadAnswer,
    /// 409 `resolve_unsupported`: connected, but its build does not
    /// announce `Capability::ResolvePath` (ACP core §3.3: the collector
    /// never sends a frame that needs a capability to a host that lacks
    /// it).
    Unsupported,
}

impl IntoResponse for NotResolved {
    fn into_response(self) -> Response {
        match self {
            Self::HostOffline => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
            // The host's own text is shown only within bounds (6c's A6): a
            // host that cannot be trusted with its workspace roots is not
            // trusted with the bytes it puts in an error either.
            Self::Refused { code, message } if code == "invalid" => {
                let message = if is_displayable_text(&message, 256) {
                    message
                } else {
                    "the host refused the path".to_string()
                };
                error(StatusCode::BAD_REQUEST, "invalid", message)
            }
            Self::Refused { code, .. } if code == "busy" => Self::Busy.into_response(),
            Self::Refused { code, message } => {
                tracing::warn!(
                    ?code,
                    ?message,
                    "resolve_path refused with a code it has no business with"
                );
                error(StatusCode::BAD_GATEWAY, "host_refused", "the host refused the request")
            }
            Self::NoAnswer => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "no_answer",
                "the host did not answer in time",
            ),
            Self::BadAnswer => error(
                StatusCode::BAD_GATEWAY,
                "bad_host_answer",
                "the host's answer is not a canonical path",
            ),
            Self::Unsupported => error(
                StatusCode::CONFLICT,
                "resolve_unsupported",
                "this host cannot resolve paths; update it",
            ),
            Self::Busy => error(StatusCode::SERVICE_UNAVAILABLE, "busy", "the host is busy; try again"),
        }
    }
}

/// Ask `host_id` to resolve `path`, a probe (ACP core §3.3): only to a
/// host that announced `resolve_path` (`Hub::probe` checks it), with the
/// probes' own timeout, which keeps the connection.
pub(crate) async fn resolve_on_host(state: &AppState, host_id: &str, path: &str) -> Result<OnHost, NotResolved> {
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ResolvePath {
        request_id: request_id.clone(),
        path: path.to_string(),
    };
    match state.hub.probe(host_id, &request_id, frame, state.probe_timeout).await {
        Ok(HostFrame::ResolvedPath {
            canonical,
            exists,
            is_dir,
            ..
        }) if is_canonical(&canonical) => Ok(OnHost {
            canonical,
            exists,
            is_dir,
        }),
        Ok(other) => {
            tracing::warn!(
                %host_id,
                answer = ?other,
                "resolve_path answered with a path not in canonical form, or with another frame"
            );
            Err(NotResolved::BadAnswer)
        }
        Err(RequestError::NotConnected) => Err(NotResolved::HostOffline),
        Err(RequestError::Rejected { code, message }) => Err(NotResolved::Refused { code, message }),
        Err(RequestError::DeliveryUnknown) => Err(NotResolved::NoAnswer),
        Err(RequestError::Unsupported) => Err(NotResolved::Unsupported),
        Err(RequestError::Busy) => Err(NotResolved::Busy),
    }
}
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-testkit --locked --test resolve --test hats`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `resolve_on_host`, drop the `if is_canonical(&canonical)` guard. `an_answer_not_in_canonical_form_is_refused` fails.
- In `NotResolved::into_response`, pass every refusal's code through. `a_refusal_an_offline_host_and_a_lost_connection_each_answer_plainly` fails on `host_refused`.
- In `rules_through_host`, store an unresolved prefix when its host is away. `path_rules_are_saved_only_with_their_host_connected` fails.
- In `replace_path_rules`, drop the rule limit before the probes. The connected-host test fails on its over-limit set.
- In `rules_through_host`, set `verified: true` for every prefix. `a_connected_host_resolves_and_verifies_rule_prefixes` fails.

- [ ] **Step 6: The full checks**

Run the five commands. Expected: all pass; **804 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates schema web
git commit -m "feat(sessions): POST /api/hats/resolve, and rules saved only through their host"
```

## After this plan

**What the frontend must do (plan 4):**
- **New session and the hat tester:**
  - resolve the typed path with `POST /api/hats/resolve`, and show the canonical path and its hat;
  - render paths escaped, since a host's paths may hold bidi or zero-width characters;
  - on 409 `resolve_unsupported`, say the host needs an update.
- **Path rules:**
  - saving needs the host connected (409 `host_offline`);
  - show `verified`: unverified means the path did not exist when it was saved.
- **Answers:**
  - 502 `bad_host_answer` and 502 `host_refused` mean the host misbehaved; show its message as text;
  - 503 `no_answer` and 503 `busy` mean retry.

**Obligations this plan hands on:**
- **5c, sessions carry their hat:**
  - resolve the cwd with `resolve_on_host` at start and resume;
  - refuse a near miss;
  - compare the hat atomically in `request_resume`;
  - add the host's own canonical-cwd check at attach;
  - `POST /api/hats/resolve` uses `session_hat` (P1);
  - switch `project_recents` to the stored `sessions.hat_id` (6c's A3 iii);
  - wire the session list's `?hat=` (6b's seam);
  - start must refuse `!exists || !is_dir`.
- **6c's suggested refactors** (in 6c's "After this plan"): one home for the path constants (`MAX_PATH` is now defined four times, `paths.rs` included); a probe-route helper; a `ProbeError` type.
- **Re-verifying unverified rules** when their host connects: not done. A rule stays unverified until the set is saved again with its directory there.
- **Host error codes elsewhere** (P5, recorded): `request_failed` still passes host-chosen codes through under a 502 on start, resume and the session requests.
- **The gateway (plan 8):** a host can pick, by lying about its own filesystem, any hat among its default and its rules' hats (decision 7).

**Not tested here:**
- **Other filesystems:** macOS firmlinks, Linux casefold directories, vfat and exfat. Bind mounts give one directory two canonical paths; documented, not handled.
- **A missing tail's case or Unicode form** on a case- or normalisation-insensitive volume, when the directory is later made in another form: the rule stays unverified, and 5c's near miss is what catches it.
- **The Linux side:** only macOS compiled here; CI's `ubuntu-latest` runs the permission test as a non-root user.

**Spec amendments** (written back in this plan's record commit, B3):
- decisions 3 and 5: kernel §5.2;
- decisions 1 and 2: kernel §5.4 and ACP core §3.3 (the frames and the `resolve_path` capability);
- decision 6: kernel §8 (`exists`, `is_dir`).

**The security review's answers** (2026-10-02, for 5b):
1. **Probes:** right. A host cannot answer another's probe. A slow host costs only its own connection, and a wrong one is held to `is_canonical`. Host error codes are P5.
2. **Host resolution:** right. `~` expands only to the host's home, and a non-UTF-8 path is refused. Task 1's fix round tightened the missing tail: a `..` past the existing part is refused.

Then, in order:
- **(5c) Sessions carry their hat**
- **(5d) Re-assignment**
- **(4) Frontend shell**
- **(8) Gateway**

---

_Generated with Claude AI — please review before distribution._
