# Hats (plan 5b): `resolve_path` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** Paths are resolved where the filesystem is: on the host (kernel spec §5.2, §5.4; umbrella §8.2).
- The `resolve_path` request and its `resolved_path` reply are on the wire. The host canonicalises a typed path: absolute or `~/…`, symlinks resolved, no `.` or `..`, no trailing slash.
- The hub gains probes: requests answered by a reply frame of their own, answered only by the host connection they went out on.
- `POST /api/hats/resolve` reports the canonical path and the hat it resolves to.
- `PUT /api/hosts/{id}/path-rules` resolves every prefix through the host, so rules are saved only with the host connected.

No session carries a hat yet: that is plan 5c. Re-assignment is plan 5d.

**Architecture:**
- **Host** (`hennery-host`):
  - `paths.rs` (new): `resolve(typed, home)`, pure and tested;
  - `connection.rs`: answers `resolve_path` from a blocking thread, at most four at once.
- **Wire** (`hennery-proto`): `CollectorFrame::ResolvePath`, `HostFrame::ResolvedPath`, `HatResolveRequest` and `HatResolution`.
- **Sessions** (`hennery-sessions`):
  - `hub.rs`: `probe`, `probe_reply` and `reject_probe`. The probes are a map apart from the request waiters, scoped to host and connection, with a hub-owned deadline.
  - `ws.rs`: routes replies and rejections to the probes.
  - `resolve.rs` (new): `resolve_on_host`, which checks the answer's form.
  - `hats.rs`: `POST /api/hats/resolve`, and the rules resolved through the host.
- **Tests:**
  - the host's resolver: real symlinks, `~`, a missing tail, a dangling symlink, case on macOS;
  - the hub's probes;
  - over HTTP, against a scripted host (wrong answers on purpose) and a real host (real symlinks).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md), the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md) and [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md), these sections:
- kernel §5.2: "Canonicalisation happens **on the host**, which is where the filesystem is (`resolve_path` request, §5.4). Rule prefixes are stored canonicalised the same way (resolved through the host when the rule is saved; a rule for a path that does not exist is stored as typed, normalised lexically, and marked unverified)."
- kernel §5.4: "`resolve_path{path}` → `resolved_path{canonical, exists, is_dir}` (or `error`) is part of the frame catalogue (ACP core §3.3). It is used for typed paths in New session, for session start and resume, for rule saving, and by the hat tester."
- kernel §8: `POST /api/hats/resolve` takes `{host_id, path}` and answers `{canonical, hat_id, rule_id?}`.
- umbrella §8.2: paths are "canonicalised **on the host** (absolute, symlinks resolved, `.`/`..` removed, no trailing slash) before matching".
- ACP core §3.3, the frame catalogue; §3.4: every request outlives the read deadline, and a timeout drops the connection.

It builds on the executed [plan 5a](2026-10-06-hats.md). Read its "After this plan" first: the 5b obligations there are this plan's scope. Every anchor below was taken from `main` at `58624d7`, which merged PR #38 (5a). Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** not executed; amended after the security review.

The security review of 2026-10-02, binding on the maintainer's behalf, covered plans 5b, 5c and 5d together. It approved after amendments (B1–B3 required; P1 and P4–P7 taken; P2 and P3 recorded; see "Decisions", "What the review changed"). It then re-confirmed the amended code: "confirmed with notes".

**How the code blocks were made and checked:**
- Every block below was generated from the amended code.
- The plan was then replayed from its own text onto `58624d7`. After each task's Step 1 the tree matched the scratch's tests-only commit, and after each task the task commit, byte for byte, the generated files included.
- After every task the five checks of "Global Constraints" passed. The tests rose from 581: 586 after Task 1, 590 after Task 2, 595 after Task 3.
- The security guards were revert-probed (each task's "Revert-probes" step).
- Each "Expected:" of the failing-test and passing-test steps is what the run printed.

## Scope

5a hands 5b these items ("After this plan"):
- the typed prefix goes through the host's `resolve_path` first, where `~`, `..` and symlinks are resolved;
- `verified` comes only from the host's answer;
- the host refuses a canonical form that is not UTF-8, and the collector accepts an answer only in canonical form, at most 4096 bytes long;
- a missing path's deepest existing ancestor is resolved;
- two typed prefixes that resolve to one are refused as a set;
- the measurements of other filesystems, which are recorded.

That is **3 tasks**:
1. the host's path resolver;
2. `resolve_path` on the wire, and the hub's probes;
3. `POST /api/hats/resolve`, and rules resolved through their host.

**In:**
- Host: `paths.rs` (Task 1); the `resolve_path` handler (Task 2).
- Wire: the frames (Task 2); the resolve types (Task 3).
- Sessions: the probes and their routing (Task 2); `resolve.rs` and the routes (Task 3).

**Out** (see "After this plan"):
- sessions carrying a hat (5c), and re-assignment (5d);
- the projects probes (`list_projects`, `browse_directory`), which are plan 6c's and reuse the probe map;
- re-verifying rules on connect.

## Decisions this plan makes where the spec is silent

A stronger-model security review, made on the maintainer's behalf on 2026-10-02, confirmed these: "approve after amendments", then "confirmed with notes". Decisions the review changed are marked "(amended after the security review of 2026-10-02)". Items marked **(amendment)** depart from explicit spec text and should be written back into it.

**What the review changed (for 5b):**
- B1: rules are saved only with their host connected. Any non-empty set for a host that is away is 409 `host_offline`, and the lexical fallback is gone from the route (decision 5). By its text alone, a prefix under a symlinked parent misses every session under it, silently: `/tmp` and `/var` on macOS, `/home` on Fedora Atomic, a home on another volume.
- B3: the spec write-back ("Spec amendments").
- Optional hardening taken:
  - P5: a host error code other than `invalid` is answered 502 `host_refused` (decision 4). A host must not make the collector say, for instance, `step_up_required`.
  - P6: the host resolves at most four paths at once (decision 3).
  - P7: a dangling symlink in a missing path's tail is refused (decision 3).

1. **The frames.**
   - `CollectorFrame::ResolvePath{request_id, path}` and `HostFrame::ResolvedPath{request_id, canonical, exists, is_dir}`.
   - The reply is not outboxed: a probe changes nothing on the host, so a lost reply costs only a retry.
   - A refusal is `HostFrame::Error{request_id, code: "invalid", message}`.
2. **The hub's probes.**
   - A map of their own, apart from the request waiters, whose answer is a `SessionBody`.
   - A probe is answered only by the host and connection it went out on (`probe_reply` and `reject_probe` take both), the rule final review M1 set for turns.
   - Its deadline is the hub's, so a probe whose HTTP handler is gone still ends. At the deadline the connection is kicked, as for every request (ACP core §3.4).
   - A late reply is logged and dropped. `unregister` fails a connection's probes as `DeliveryUnknown`.
   - The names (`Hub::probe`, `probe_reply`, one `ws.rs` arm per reply frame) were agreed with the plan 6 lane, whose project probes reuse them.
3. **The host's resolution** (`paths::resolve`). (amended after the security review of 2026-10-02: P6, P7)
   - `~` and `~/…` are the host user's `$HOME`. `~user`, a relative path, a control character, more than 4096 bytes, and a canonical form that is not UTF-8 are refused. Nothing is converted lossily: two paths could become one string.
   - An existing path goes through `std::fs::canonicalize`. On macOS that gives the case on disk, measured by a test that skips itself on a case-sensitive volume.
   - A missing path keeps its deepest existing ancestor canonicalised, and the rest is applied by its text (amendment of "stored as typed"). Its `..` is applied lexically only past the existing part, where it is the real parent.
   - The first missing component must not be a dangling symlink (P7): its name would stand in the prefix, and once the target exists, paths would resolve to the target instead.
   - It runs on a blocking thread, at most four at once (P6).
4. **The collector checks every answer.** (amended after the security review of 2026-10-02: P5)
   - `resolved_path.canonical` is accepted only if `is_canonical` holds (5a). Otherwise the answer is 502 `bad_host_answer`, and the reply is logged escaped.
   - A refusal coded `invalid` is 400 `invalid` with the host's message. Any other code is 502 `host_refused`.
   - An offline host is 409 `host_offline`. A lost connection or a timeout is 503 `delivery_unknown`.
   - `RESOLVE_TIMEOUT` is 50 s, above the 45 s read deadline, pinned by a const assert.
5. **Rules are resolved through their host, or not saved.** (amended after the security review of 2026-10-02: B1) (amendment)
   - `PUT /api/hosts/{id}/path-rules` checks that the host exists (404), then sends every typed prefix to the host: `~`, `..` and symlinks included, at most eight at once, in the order given.
   - `verified` is the host's `exists`. A missing prefix keeps its resolved ancestor (decision 3).
   - The first refusal is the answer, and nothing is stored. With the host away, a non-empty set is 409 `host_offline`; the empty set clears the rules without the host.
   - Two prefixes that resolve to one are refused as a set by 5a's duplicate check. Examples: `/p/Acme` and `/p/acme` on APFS, or `/tmp/x` and `/private/tmp/x`.
   - Kernel §5.2's "stored as typed" no longer happens: an unverified rule is one whose path did not exist when it was saved.
6. **`POST /api/hats/resolve`** takes `{host_id, path}` and answers `{canonical, exists, is_dir, hat_id, rule_id?}`.
   - It needs the operator but no step-up: it changes nothing.
   - In 5c it starts refusing `hat_ambiguous` where a session start would (the review's P1).
7. **What a host can do.** The host is authenticated, but it is trusted only for its own filesystem. It can lie about which directory a path is, and so pick any hat among its default and its rules' hats. That is within umbrella §8.4's threat model; recorded for the gateway.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `nix develop -c cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check`.
- **No new crates;** `Cargo.lock` does not change.
- **Wire types change in Tasks 2 and 3.** The generated files are regenerated there with `cargo run -p hennery-proto --bin gen`; in `codegen.rs`, new root types go in both lists.
- Every timeout of a request to a host exceeds the connection's read deadline (ACP core §3.4).
- Every new route is an operator's (`operator_only`), and takes its body as `ApiJson`.
- **No Linux-only code**, and no test that reads another process's state without polling for a positive signal (fleet rule). CI's `ubuntu-latest` is the only Linux check; only macOS was compiled here.
- No global installs: tooling comes from the flake dev shell.
- Commits follow Conventional Commits and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **Paths under a symlink, as macOS gives them** (`/tmp`, `/var/folders/…`), for rules and for the hat tester.
   - Expected: the canonical form, the rule verified when it exists, and the hat its rules give.
   - Tests: Task 1 `an_existing_path_is_canonicalised_through_its_symlinks`, `a_missing_path_keeps_its_existing_ancestor_resolved`; Task 3 `a_real_host_resolves_symlinks_for_the_resolver_and_for_rules`.
2. **A host that answers wrongly, refuses oddly, or goes away mid-probe.**
   - Expected: 502 `bad_host_answer` for a path not in canonical form; 400 `invalid` or 502 `host_refused` for a refusal; 503 `delivery_unknown` for a lost connection. No probe left behind.
   - Tests: Task 3 `an_answer_not_in_canonical_form_is_refused`, `a_refusal_an_offline_host_and_a_lost_connection_each_answer_plainly`; Task 2's hub tests.
3. **Another host answering a probe it was not sent.**
   - Expected: it completes nothing; the right connection's reply does.
   - Tests: Task 2 `a_probe_is_answered_only_by_its_own_hosts_connection`.
4. **Saving rules while the host is away, or rules that collapse into one.**
   - Expected: 409 `host_offline` and nothing stored (the empty set clears them); 400 for two prefixes resolving to one.
   - Tests: Task 3 `path_rules_are_saved_only_with_their_host_connected`, `a_connected_host_resolves_and_verifies_rule_prefixes`.
5. **Typed paths that are not quite paths:** `~`, `~other`, relative paths, control characters, a dangling symlink, an over-long path.
   - Expected: `~` is the host's home; everything else is refused with a reason.
   - Tests: Task 1 `a_tilde_is_the_hosts_home_and_nothing_else_is_relative`, `a_dangling_symlink_is_refused_not_kept_by_its_name`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-host/src/paths.rs` (new), `lib.rs` | The resolver | 1 |
| `crates/hennery-proto/src/frames.rs`, generated files | The frames | 2 |
| `crates/hennery-host/src/connection.rs` | The `resolve_path` handler | 2 |
| `crates/hennery-sessions/src/hub.rs`, `ws.rs` | Probes and their routing | 2 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs` | The resolve types | 3 |
| `crates/hennery-sessions/src/resolve.rs` (new), `lib.rs`, `hats.rs` | `resolve_on_host`, the routes | 3 |
| Tests: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-sessions/tests/hub.rs`, `crates/hennery-testkit/tests/resolve.rs` (new), `crates/hennery-testkit/tests/hats.rs` (rules need their host) | | 2, 3 |

All commands run from the repository root inside the dev shell (`nix develop -c …`). Work on a feature branch off `main` (e.g. `feat/hats-5b`). Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "Replace the whole of `path` with:" overwrites the file with the block.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate: `cargo run -p hennery-proto --bin gen` rewrites the generated files, and changes no other file.

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
        Err(_) => (resolve_missing(&path)?, false),
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
/// first of those must not be a symlink whose target is missing (the
/// review's P7): its name would stand in the result, and once the target
/// exists, paths under it would resolve to the target instead.
fn resolve_missing(path: &Path) -> Result<PathBuf, String> {
    let components: Vec<Component<'_>> = path.components().collect();
    for split in (1..components.len()).rev() {
        let ancestor: PathBuf = components[..split].iter().collect();
        let Ok(mut out) = std::fs::canonicalize(&ancestor) else {
            continue;
        };
        if let Component::Normal(name) = components[split]
            && out.join(name).symlink_metadata().is_ok()
        {
            return Err(format!(
                "{:?} is a symlink whose target does not exist",
                out.join(name).display()
            ));
        }
        for component in &components[split..] {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
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
        let typed = format!("{}/link/not/yet/../made/", dir.path().display());
        assert_eq!(
            resolve(&typed, None),
            Ok(Resolved {
                canonical: format!("{root}/real/not/made"),
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

Run: `cargo test -p hennery-host --locked --lib paths`
Expected: PASS, 6 tests (the case test returns early on a case-sensitive volume).

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `resolve_missing`, remove the dangling-symlink check. `a_dangling_symlink_is_refused_not_kept_by_its_name` fails.
- In `expand_home`, accept `Some(rest)` as a path under the home whatever follows the `~`. `a_tilde_is_the_hosts_home_and_nothing_else_is_relative` fails on `~other/Projects`.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **586 tests** in the workspace.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-host
git commit -m "feat(host): resolve a typed path where the filesystem is"
```

### Task 2: `resolve_path` on the wire, and the hub's probes

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`, `crates/hennery-host/src/connection.rs`, `crates/hennery-sessions/src/hub.rs`, `crates/hennery-sessions/src/ws.rs`, the generated files
- Test: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-sessions/tests/hub.rs`

**Interfaces:**
- Produces:
  - `CollectorFrame::ResolvePath{request_id, path}` and `HostFrame::ResolvedPath{request_id, canonical, exists, is_dir}`;
  - `Hub::probe(host_id, request_id, frame, timeout) -> Result<HostFrame, RequestError>`;
  - `Hub::probe_reply(host_id, conn_id, request_id, frame) -> bool` and `Hub::reject_probe(host_id, conn_id, request_id, code, message) -> bool`;
  - `pending_requests` counts probes.
- Consumes: Task 1's `paths::resolve`.

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
        assert!(serde_json::from_value::<AnswerRequest>(bad.clone()).is_err(), "{bad}");
    }
}
```

with:

```rust
        assert!(serde_json::from_value::<AnswerRequest>(bad.clone()).is_err(), "{bad}");
    }
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

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust

use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, ParkReason, SessionBody, TurnOutcome};
```

with:

```rust
//! Probes (plan 5b) too, and only their own host answers them.

use hennery_proto::frames::{
    Capabilities, Capability, CollectorFrame, HostFrame, ParkReason, SessionBody, TurnOutcome,
};
```

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
    assert_eq!(hub.pending_requests(), 0);
}
```

with:

```rust
    assert_eq!(hub.pending_requests(), 0);
}

fn resolve_path(request_id: &str) -> CollectorFrame {
    CollectorFrame::ResolvePath {
        request_id: request_id.into(),
        path: "~/p".into(),
    }
}

fn resolved(request_id: &str) -> HostFrame {
    HostFrame::ResolvedPath {
        request_id: request_id.into(),
        canonical: "/home/me/p".into(),
        exists: true,
        is_dir: true,
    }
}

/// Plan 5b decision 2: a probe is answered only by the connection it went
/// out on. Another host naming its request id, or the same host's old
/// connection, completes nothing (final review M1's rule for turns).
#[tokio::test]
async fn a_probe_is_answered_only_by_its_own_hosts_connection() {
    let hub = Arc::new(Hub::new());
    let (reg, mut rx) = connect(&hub);
    let (other_tx, _other_rx) = mpsc::unbounded_channel();
    let other = hub.register("other", other_tx, Capabilities::default()).unwrap();
    hub.mark_ready("other", other.conn_id);
    let probe = tokio::spawn({
        let hub = hub.clone();
        async move { hub.probe("h", "p1", resolve_path("p1"), Duration::from_secs(5)).await }
    });
    assert_eq!(rx.recv().await, Some(resolve_path("p1")));

    assert!(!hub.probe_reply("other", other.conn_id, "p1", resolved("p1")));
    assert!(!hub.reject_probe("other", other.conn_id, "p1", "invalid".into(), "no".into()));
    assert!(!hub.probe_reply("h", reg.conn_id + 1000, "p1", resolved("p1")));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!probe.is_finished(), "answered by a connection it did not go out on");

    assert!(hub.probe_reply("h", reg.conn_id, "p1", resolved("p1")));
    assert_eq!(probe.await.unwrap(), Ok(resolved("p1")));
    // Late: nobody waits any more.
    assert!(!hub.probe_reply("h", reg.conn_id, "p1", resolved("p1")));
    assert_eq!(hub.pending_requests(), 0);
}

#[tokio::test]
async fn a_probe_is_rejected_refused_offline_and_failed_with_its_connection() {
    let hub = Arc::new(Hub::new());
    assert_eq!(
        hub.probe("h", "p0", resolve_path("p0"), Duration::from_secs(5)).await,
        Err(RequestError::NotConnected)
    );
    let (reg, mut rx) = connect(&hub);
    let probe = tokio::spawn({
        let hub = hub.clone();
        async move { hub.probe("h", "p1", resolve_path("p1"), Duration::from_secs(5)).await }
    });
    rx.recv().await.unwrap();
    assert!(hub.reject_probe("h", reg.conn_id, "p1", "invalid".into(), "relative".into()));
    assert_eq!(
        probe.await.unwrap(),
        Err(RequestError::Rejected {
            code: "invalid".into(),
            message: "relative".into()
        })
    );

    // At once, not at its deadline.
    let probe = tokio::spawn({
        let hub = hub.clone();
        async move { hub.probe("h", "p2", resolve_path("p2"), Duration::from_secs(600)).await }
    });
    rx.recv().await.unwrap();
    hub.unregister("h", reg.conn_id);
    let failed = tokio::time::timeout(Duration::from_secs(5), probe)
        .await
        .expect("the probe was not failed with its connection");
    assert_eq!(failed.unwrap(), Err(RequestError::DeliveryUnknown));
    assert_eq!(hub.pending_requests(), 0);
}

/// ACP core §3.4, as for requests: a probe nobody answers drops the
/// connection at its deadline, even when its caller is gone.
#[tokio::test]
async fn a_dropped_probes_deadline_still_kicks_the_connection_and_frees_its_waiter() {
    let hub = Arc::new(Hub::new());
    let (reg, mut rx) = connect(&hub);
    let probe = tokio::spawn({
        let hub = hub.clone();
        async move {
            hub.probe("h", "p1", resolve_path("p1"), Duration::from_millis(200))
                .await
        }
    });
    rx.recv().await.unwrap();
    probe.abort();
    tokio::time::timeout(Duration::from_secs(5), reg.kicked.cancelled())
        .await
        .expect("the connection was not kicked");
    assert_eq!(hub.pending_requests(), 0);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-proto --locked --test frames`
Expected: FAIL to compile: `no variant named ResolvePath found for enum CollectorFrame`, `no variant named ResolvedPath found for enum HostFrame`.

- [ ] **Step 3: The frames, the host's answer, and the probes**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        ),
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

with:

```rust
        ),
        CollectorFrame::ResolvePath { request_id, path } => resolve_path(uplink, request_id, path),
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust

/// An operator's answer goes to the session's live actor, which reports
```

with:

```rust

/// Answer `resolve_path` (kernel spec §5.4) from a blocking thread: the
/// filesystem may be slow (a network mount), and the connection loop must
/// not wait on it.
fn resolve_path(uplink: &Uplink, request_id: String, path: String) {
    // At most this many at once (the review's P6): a slow mount must not
    // pile up blocking threads; the collector drops the connection anyway.
    static RESOLVING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);
    let uplink = uplink.clone();
    tokio::spawn(async move {
        let Ok(_slot) = RESOLVING.acquire().await else {
            return;
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let resolved = tokio::task::spawn_blocking(move || crate::paths::resolve(&path, home.as_deref())).await;
        let frame = match resolved {
            Ok(resolved) => resolved_path_frame(request_id, resolved),
            // The resolution panicked: the collector's timeout answers.
            Err(_) => return,
        };
        uplink.reply(frame);
    });
}

/// The answer to `resolve_path`: the resolved path, or `invalid` and why.
fn resolved_path_frame(request_id: String, resolved: Result<crate::paths::Resolved, String>) -> HostFrame {
    match resolved {
        Ok(resolved) => HostFrame::ResolvedPath {
            request_id,
            canonical: resolved.canonical,
            exists: resolved.exists,
            is_dir: resolved.is_dir,
        },
        Err(message) => HostFrame::Error {
            request_id,
            code: "invalid".into(),
            message,
        },
    }
}

/// An operator's answer goes to the session's live actor, which reports
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

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust

struct HostConn {
```

with:

```rust

/// A probe in flight (`Hub::probe`): a request whose answer is a reply
/// frame of its own, not an outboxed fact (kernel spec §5.4). Only the
/// connection it went out on can answer it.
struct ProbeWaiter {
    host_id: String,
    conn_id: u64,
    tx: oneshot::Sender<Result<HostFrame, RequestError>>,
}

struct HostConn {
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    waiters: Arc<Mutex<HashMap<String, Waiter>>>,
    events: broadcast::Sender<EventDto>,
```

with:

```rust
    waiters: Arc<Mutex<HashMap<String, Waiter>>>,
    /// Probes in flight, by request id.
    probes: Arc<Mutex<HashMap<String, ProbeWaiter>>>,
    events: broadcast::Sender<EventDto>,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
            waiters: Arc::new(Mutex::new(HashMap::new())),
```

with:

```rust
            waiters: Arc::new(Mutex::new(HashMap::new())),
            probes: Arc::new(Mutex::new(HashMap::new())),
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
                let _ = w.tx.send(Err(RequestError::DeliveryUnknown));
            }
        }
    }

```

with:

```rust
                let _ = w.tx.send(Err(RequestError::DeliveryUnknown));
            }
        }
        drop(waiters);
        let mut probes = self.probes.lock().expect("probes lock");
        let ids: Vec<String> = probes
            .iter()
            .filter(|(_, p)| p.conn_id == conn_id)
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(p) = probes.remove(&id) {
                let _ = p.tx.send(Err(RequestError::DeliveryUnknown));
            }
        }
    }

```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust

    pub fn resolve(&self, request_id: &str, fact: SessionBody) {
```

with:

```rust

    /// Send a probe (`resolve_path`) to a host that is connected and
    /// reconciled, and wait for its reply frame (`probe_reply`), its
    /// rejection (`reject_probe`), the connection's end, or `timeout`. A
    /// probe changes nothing on the host, but its timeout is treated like
    /// any request's (ACP core §3.4): a live connection that answers neither
    /// way is dropped. A reply that comes after is logged and dropped.
    pub async fn probe(
        &self,
        host_id: &str,
        request_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<HostFrame, RequestError> {
        let (tx, rx) = oneshot::channel();
        let (conn_id, kicked) = {
            let hosts = self.hosts.lock().expect("hosts lock");
            let Some(host) = hosts.get(host_id).filter(|h| h.routable()) else {
                return Err(RequestError::NotConnected);
            };
            self.probes.lock().expect("probes lock").insert(
                request_id.to_string(),
                ProbeWaiter {
                    host_id: host_id.to_string(),
                    conn_id: host.conn_id,
                    tx,
                },
            );
            if host.tx.send(frame).is_err() {
                self.probes.lock().expect("probes lock").remove(request_id);
                return Err(RequestError::NotConnected);
            }
            (host.conn_id, host.kicked.clone())
        };
        // As for requests: the deadline is the hub's, so a probe whose
        // handler is gone (its client disconnected) still ends.
        let watchdog = tokio::spawn(expire_probe(
            self.probes.clone(),
            host_id.to_string(),
            request_id.to_string(),
            conn_id,
            kicked,
            timeout,
        ));
        let result = tokio::time::timeout(timeout, rx).await;
        watchdog.abort();
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(RequestError::DeliveryUnknown),
            Err(_elapsed) => {
                if self.probes.lock().expect("probes lock").remove(request_id).is_some() {
                    tracing::warn!(%host_id, %request_id, "probe timed out; dropping the host connection");
                    self.disconnect_conn(host_id, conn_id);
                }
                Err(RequestError::DeliveryUnknown)
            }
        }
    }

    /// A reply frame for a probe, from `host_id`'s connection `conn_id`.
    /// `false` if no probe of that connection waits for it: a late reply,
    /// or one naming another host's probe.
    pub fn probe_reply(&self, host_id: &str, conn_id: u64, request_id: &str, frame: HostFrame) -> bool {
        match self.take_probe(host_id, conn_id, request_id) {
            Some(p) => {
                let _ = p.tx.send(Ok(frame));
                true
            }
            None => false,
        }
    }

    /// A rejection (`error`) of a probe, scoped like `probe_reply`.
    pub fn reject_probe(&self, host_id: &str, conn_id: u64, request_id: &str, code: String, message: String) -> bool {
        match self.take_probe(host_id, conn_id, request_id) {
            Some(p) => {
                let _ = p.tx.send(Err(RequestError::Rejected { code, message }));
                true
            }
            None => false,
        }
    }

    fn take_probe(&self, host_id: &str, conn_id: u64, request_id: &str) -> Option<ProbeWaiter> {
        let mut probes = self.probes.lock().expect("probes lock");
        let ours = probes
            .get(request_id)
            .is_some_and(|p| p.host_id == host_id && p.conn_id == conn_id);
        ours.then(|| probes.remove(request_id)).flatten()
    }

    pub fn resolve(&self, request_id: &str, fact: SessionBody) {
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    /// Requests still waiting for their answer.
    pub fn pending_requests(&self) -> usize {
        self.waiters.lock().expect("waiters lock").len()
```

with:

```rust
    /// Requests still waiting for their answer, probes included.
    pub fn pending_requests(&self) -> usize {
        self.waiters.lock().expect("waiters lock").len() + self.probes.lock().expect("probes lock").len()
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust

/// One waiter's deadline, owned by the hub. If the waiter is still there
```

with:

```rust

/// One probe's deadline, owned by the hub, like `expire`'s for a request.
async fn expire_probe(
    probes: Arc<Mutex<HashMap<String, ProbeWaiter>>>,
    host_id: String,
    request_id: String,
    conn_id: u64,
    kicked: CancellationToken,
    timeout: Duration,
) {
    tokio::time::sleep(timeout).await;
    let expired = {
        let mut probes = probes.lock().expect("probes lock");
        match probes.get(&request_id) {
            Some(p) if p.conn_id == conn_id => probes.remove(&request_id),
            _ => None,
        }
    };
    if let Some(p) = expired {
        tracing::warn!(%host_id, %request_id, "probe timed out with no handler left; dropping the host connection");
        kicked.cancel();
        let _ = p.tx.send(Err(RequestError::DeliveryUnknown));
    }
}

/// One waiter's deadline, owned by the hub. If the waiter is still there
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
            }
            HostFrame::Error {
```

with:

```rust
            }
            // A probe's reply (kernel spec §5.4): only to a probe this
            // connection was sent.
            HostFrame::ResolvedPath { ref request_id, .. } => {
                let request_id = request_id.clone();
                if !state.hub.probe_reply(&host_id, conn_id, &request_id, frame) {
                    tracing::warn!(%host_id, %request_id, "a probe reply nobody waits for");
                }
            }
            HostFrame::Error {
                request_id,
                code,
                message,
            } if state
                .hub
                .reject_probe(&host_id, conn_id, &request_id, code.clone(), message.clone()) => {}
            HostFrame::Error {
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-proto --locked --test frames && cargo test -p hennery-sessions --locked --test hub`
Expected: PASS: `frames` 19, `hub` 11.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `take_probe`, drop the `p.host_id == host_id &&` condition. `a_probe_is_answered_only_by_its_own_hosts_connection` fails.
- In `probe`, remove the `expire_probe` watchdog's spawn. `a_dropped_probes_deadline_still_kicks_the_connection_and_frees_its_waiter` fails.
- In `unregister`, remove the loop that fails the connection's probes. `a_probe_is_rejected_refused_offline_and_failed_with_its_connection` fails.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **590 tests** in the workspace.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-proto crates/hennery-host crates/hennery-sessions schema web
git commit -m "feat(sessions): resolve_path on the wire, and probes answered only by their own connection"
```

### Task 3: `POST /api/hats/resolve`, and rules resolved through their host

**Files:**
- Create: `crates/hennery-sessions/src/resolve.rs`
- Modify: `crates/hennery-sessions/src/lib.rs`, `crates/hennery-sessions/src/hats.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, the generated files
- Test: `crates/hennery-testkit/tests/resolve.rs` (new), `crates/hennery-testkit/tests/hats.rs`

**Interfaces:**
- Produces:
  - `crate::resolve::{RESOLVE_TIMEOUT, OnHost {canonical, exists, is_dir}, NotResolved, resolve_on_host(state, host_id, path) -> Result<OnHost, NotResolved>}`;
  - `POST /api/hats/resolve` `HatResolveRequest` → `HatResolution` | 400 | 404 | 409 | 502 | 503;
  - `PUT /api/hosts/{id}/path-rules` through the host.
- Consumes: Task 2's `Hub::probe`; 5a's `Hosts::{resolve_hat, replace_path_rules, path_rules}` and `is_canonical`.

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
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame};
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
    async fn connect(collector: &Collector) -> Self {
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
            capabilities: Capabilities::default(),
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
    // review's P5): it could make a client prompt for a step-up.
    let call = resolve(&collector, "/p");
    let (request_id, _) = host.resolve_request().await;
    host.send(&HostFrame::Error {
        request_id,
        code: "step_up_required".into(),
        message: "no".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (502, Some("host_refused")));

    let call = resolve(&collector, "/p");
    host.resolve_request().await;
    drop(host);
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (503, Some("delivery_unknown")));
    assert_eq!(collector.state.hub.pending_requests(), 0);
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
use hennery_kernel::hats::{HatChange, HatRecord, NewRule, PathRule, RulesChange};
use hennery_kernel::json::ApiJson;
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    CreateHatRequest, HatItem, HatResolution, HatResolveRequest, PathRuleInput, PathRuleItem, PathRulesRequest,
    UpdateHatRequest,
};

/// Rule prefixes resolved through the host at once, at most.
const RESOLVING_AT_ONCE: usize = 8;
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
/// a symlinked parent would miss every session under it, silently.
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
pub mod offline;
```

with:

```rust
pub mod offline;
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
use hennery_proto::frames::{CollectorFrame, HostFrame};
use std::time::Duration;

/// `resolve_path`'s bound: past the host connection's read deadline, as
/// every request's (ACP core §3.4).
pub(crate) const RESOLVE_TIMEOUT: Duration = Duration::from_secs(50);

const _: () = assert!(
    RESOLVE_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
    "resolve_path's timeout must exceed the host connection's read deadline"
);

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
    /// 400 `invalid` with the host's message: it refused the path. Any
    /// other code the host chose is answered `host_refused` (the review's
    /// P5): a host must not make the collector say, for instance,
    /// `step_up_required`.
    Refused { code: String, message: String },
    /// 503 `delivery_unknown`.
    DeliveryUnknown,
    /// 502 `bad_host_answer`: an answer not in canonical form.
    BadAnswer,
}

impl IntoResponse for NotResolved {
    fn into_response(self) -> Response {
        match self {
            Self::HostOffline => error(StatusCode::CONFLICT, "host_offline", "the host is not connected"),
            Self::Refused { code, message } if code == "invalid" => error(StatusCode::BAD_REQUEST, "invalid", message),
            Self::Refused { code, message } => {
                tracing::warn!(?code, "resolve_path refused with a code it has no business with");
                error(StatusCode::BAD_GATEWAY, "host_refused", message)
            }
            Self::DeliveryUnknown => error(
                StatusCode::SERVICE_UNAVAILABLE,
                "delivery_unknown",
                "host disconnected; delivery unknown",
            ),
            Self::BadAnswer => error(
                StatusCode::BAD_GATEWAY,
                "bad_host_answer",
                "the host's answer is not a canonical path",
            ),
        }
    }
}

/// Ask `host_id` to resolve `path`.
pub(crate) async fn resolve_on_host(state: &AppState, host_id: &str, path: &str) -> Result<OnHost, NotResolved> {
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ResolvePath {
        request_id: request_id.clone(),
        path: path.to_string(),
    };
    match state.hub.probe(host_id, &request_id, frame, RESOLVE_TIMEOUT).await {
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
            tracing::warn!(%host_id, answer = ?other, "resolve_path answered with a path not in canonical form");
            Err(NotResolved::BadAnswer)
        }
        Err(RequestError::NotConnected) => Err(NotResolved::HostOffline),
        Err(RequestError::Rejected { code, message }) => Err(NotResolved::Refused { code, message }),
        Err(RequestError::DeliveryUnknown) => Err(NotResolved::DeliveryUnknown),
    }
}
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-testkit --locked --test resolve --test hats`
Expected: PASS: `hats` 5, `resolve` 5.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `resolve_on_host`, drop the `if is_canonical(&canonical)` guard. `an_answer_not_in_canonical_form_is_refused` fails.
- In `NotResolved::into_response`, pass every refusal's code through. `a_refusal_an_offline_host_and_a_lost_connection_each_answer_plainly` fails on `host_refused`.
- In `replace_path_rules`, bring back the lexical fallback for a host that is away. `path_rules_are_saved_only_with_their_host_connected` fails.
- In `rules_through_host`, set `verified: true` for every prefix. `a_connected_host_resolves_and_verifies_rule_prefixes` fails.

- [ ] **Step 6: The full checks**

Run the five commands of "Global Constraints". Expected: all pass; **595 tests** in the workspace.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-sessions crates/hennery-proto crates/hennery-testkit schema web
git commit -m "feat(sessions): POST /api/hats/resolve, and rules saved only through their host"
```

## After this plan

**What the frontend must do (plan 4):**
- **New session and the hat tester:** resolve the typed path with `POST /api/hats/resolve` and show the canonical path and its hat. Render the path escaped: a host's paths may hold bidi or zero-width characters.
- **Path rules:** saving needs the host connected (409 `host_offline`). Say so, and offer to retry. Show `verified`: unverified means the path did not exist when it was saved.
- **Answers:**
  - 502 `bad_host_answer` and 502 `host_refused` mean the host misbehaved; show its message as text.
  - 503 `delivery_unknown` means retry.

**Obligations this plan hands on:**
- **5c, sessions carry their hat:** resolve the cwd with `resolve_on_host` at start and resume; refuse a near miss; compare the hat atomically in `request_resume`; and add the host's own canonical-cwd check at attach.
- **5c:** `POST /api/hats/resolve` uses `session_hat` and refuses `hat_ambiguous` where a start would (the review's P1).
- **Plan 6c (projects):** `list_projects` and `browse_directory` reuse `Hub::probe`, with a `ws.rs` arm per reply frame and the same host and connection scoping.
- **Re-verifying unverified rules** when their host connects is not done: a rule stays unverified until the set is saved again with its directory there.
- **Host error codes elsewhere** (the review's P5, recorded): `request_failed` still passes host-chosen codes through under a 502 on start, resume and the session requests. Map unknown codes to `host_refused` there too.
- **The gateway (plan 8):** a host can pick, by lying about its own filesystem, any hat among its default and its rules' hats (decision 7). Write it next to `MountPolicy`.

**Not tested here:**
- **Other filesystems:** macOS firmlinks (`/System/Volumes/Data`), Linux casefold directories, vfat and exfat were not measured. Bind mounts give one directory two canonical paths; documented, not handled.
- **The case test on a case-sensitive macOS volume** returns early; CI's macOS image is case-insensitive.
- **The Linux side:** only macOS compiled here; CI's `ubuntu-latest` is the first Linux run.

**Spec amendments:**
- decisions 3 and 5: kernel §5.2: rules are resolved through their host when saved, and refused while it is away (B1). A missing path keeps its deepest existing ancestor resolved. `~` expands on the host. A non-UTF-8 path is refused.
- decision 1: ACP core §3.3: `resolve_path` / `resolved_path` are on the wire now.
- decision 6: kernel §8: `POST /api/hats/resolve` also answers `exists` and `is_dir`.

**The security review's answers** (2026-10-02, for 5b):
1. **Probes:** right. A host cannot answer another's probe, and only one of the timeout and the watchdog acts. A slow host costs only its own connection; a wrong one is held to `is_canonical`. Host error codes are P5.
2. **Host resolution:** right. A missing path's tail treats `..` correctly, `~` expands only to the host's `$HOME`, and a non-UTF-8 path is refused.

Then, in order:
- **(5c) Sessions carry their hat**
- **(5d) Re-assignment**
- **(4) Frontend shell**
- **(8) Gateway**

---

_Generated with Claude AI — please review before distribution._
