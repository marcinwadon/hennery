# Skills catalogue, phase 1 (skills on hosts) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The operator uploads Agent Skills directories to the collector, assigns each to hosts, and every assigned host links them into `~/.claude/skills` and `$CODEX_HOME/skills` without any step on the machine.

**Architecture:** The kernel stores skills (archive as a BLOB), assignments and per-host applied state, with one owner-wide revision counter. The collector pushes `SkillSync` to each host after its handshake and on every change; the host fetches missing archives (`SkillFetch` / `SkillArchive`), extracts them into its data directory, links them into both agents' skill directories, and answers `SkillState`. A new `/api/skills` router and a `/skills` screen manage the catalogue.

**Tech Stack:** Rust (axum, rusqlite, tar, flate2, sha2, base64), React + TypeScript (vitest). Verification through `nix develop -c …`.

**Spec:** `docs/specs/2026-11-17-skills-hooks-catalogue.md` (phase 1 of §12; §5 limits, §6 transfer, §7 materialisation, §9 API and UI). Hook sets (§8) are phase 2 and get their own plan.

## Global Constraints

- Naming: the word `catalog` already means the runtime configuration catalogue (`Capability::CatalogProbe`, `HostFrame::Catalog`, `runtime_catalog`). This feature uses `skill`/`skills` everywhere: tables `skill_*`, frames `Skill*`, `Capability::Skills`, routes `/api/skills`, feature flag `skills`, web route `/skills`.
- Skill name: 1–64 chars, `a-z`, `0-9`, `-`; no leading, trailing or double hyphen; must equal the `name` in `SKILL.md`.
- Limits (spec §5): archive uncompressed ≤ 8 MiB (`8 << 20`), compressed ≤ 8 MiB, ≤ 512 entries, single file ≤ 2 MiB, `description` 1–1024 chars, ≤ 256 skills per owner. Entries: regular files and directories only, relative UTF-8 paths, no `..`, no absolute paths.
- Executable bits are kept only under `scripts/`; every other file is written `0644`.
- hennery never replaces, renames or removes a path it did not create. A conflicting path is reported, not resolved.
- Every write route needs step-up (`hennery_kernel::auth::require_step_up`). Every store query is owner-scoped.
- WebSocket frames are capped at `MAX_FRAME = 32 << 20` on both sides; an 8 MiB archive is ≤ 11 MiB as base64, so one `SkillArchive` frame carries a whole archive. No chunking.
- Untrusted strings in the UI are rendered through `Text` / `visible()` from `web/src/lib/text`.
- Commit style: Conventional Commits, `type(scope): summary` (`feat(sessions):`, `feat(web):`, `feat(host):`, `feat(mcp):` style scopes; use `skills` as the scope for this feature).
- Rust gates (run before every commit that touches Rust): `nix develop -c cargo fmt --all --check`, `nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings`, `nix develop -c cargo test -p <crate> --locked` for the crates touched; the full set in Task 9.
- Web gates: `pnpm --dir web typecheck`, `pnpm --dir web test` (inside `nix develop .#web -c`).

## Review Focus

1. A tar.gz whose entry path escapes the root (`../x`, `/etc/x`, or a directory entry followed by a file under a symlink it did not contain) must be refused at upload and never written on the host. Test: Task 3 (`a_path_outside_the_root_is_refused`) and Task 6 (`extraction_refuses_an_escaping_path`).
2. Two skills with the same `name` from one owner must be refused (409 `skill_exists`), because both agents key skills by directory name. Test: Task 5 (`a_duplicate_name_is_refused`).
3. A host whose `~/.claude/skills/<name>` already holds the operator's own directory (or a symlink elsewhere) must report `conflict` and leave it; a later revocation must not remove it. Tests: Task 6 (`a_foreign_path_is_a_conflict_and_survives_removal`).
4. A host that reconnects after the collector changed a skill must receive the new hash, re-fetch, and switch the link atomically; a session that already loaded the old copy is not disturbed. Test: Task 7 (`a_changed_skill_is_refetched_and_relinked`).
5. A host without `Capability::Skills` (older host) must never be sent `SkillSync`, and the UI must show that state rather than "pending" forever. Tests: Task 4 (`an_old_host_takes_no_skill_sync`), Task 8 (`a_host_without_the_capability_is_marked`).

---

### Task 1: Shared skill rules in `hennery-proto`

**Files:**
- Create: `crates/hennery-proto/src/skills.rs`
- Modify: `crates/hennery-proto/src/lib.rs` (add `pub mod skills;`)

**Interfaces:**
- Produces:
  - `pub const MAX_NAME: usize = 64; pub const MAX_DESCRIPTION: usize = 1024;`
  - `pub fn valid_name(name: &str) -> bool`
  - `pub struct Frontmatter { pub name: String, pub description: String }`
  - `pub enum FrontmatterError { Missing, Unterminated, NoName, NoDescription, DescriptionTooLong, BadName }`
  - `pub fn frontmatter(skill_md: &str) -> Result<Frontmatter, FrontmatterError>` — reads only the first `---` block, line-based `key: value`, `name` and `description` required, quotes stripped (`"…"` or `'…'`), other keys ignored. Not a YAML parser; a multi-line value makes `description` end at the line.

- [ ] **Step 1: Write the failing tests** in `skills.rs` `#[cfg(test)] mod tests`:

```rust
#[test] fn names_follow_the_agent_skills_rules() {
    for ok in ["a", "pdf-processing", "a1-b2", &"x".repeat(64)] { assert!(valid_name(ok), "{ok}"); }
    for bad in ["", "-a", "a-", "a--b", "PDF", "a_b", "a b", &"x".repeat(65), "ż"] { assert!(!valid_name(bad), "{bad}"); }
}
#[test] fn frontmatter_reads_name_and_description() {
    let fm = frontmatter("---\nname: pdf\ndescription: \"Reads PDFs. Use when asked.\"\nversion: 0.4.0\n---\n# PDF\n").unwrap();
    assert_eq!(fm.name, "pdf"); assert_eq!(fm.description, "Reads PDFs. Use when asked.");
}
#[test] fn frontmatter_must_open_the_file() { assert!(matches!(frontmatter("# no\n---\nname: a\n---"), Err(FrontmatterError::Missing))); }
#[test] fn frontmatter_needs_both_fields() {
    assert!(matches!(frontmatter("---\nname: a\n---\n"), Err(FrontmatterError::NoDescription)));
    assert!(matches!(frontmatter("---\ndescription: d\n---\n"), Err(FrontmatterError::NoName)));
    assert!(matches!(frontmatter("---\nname: A\ndescription: d\n---\n"), Err(FrontmatterError::BadName)));
    let long = format!("---\nname: a\ndescription: {}\n---\n", "d".repeat(1025));
    assert!(matches!(frontmatter(&long), Err(FrontmatterError::DescriptionTooLong)));
}
#[test] fn frontmatter_must_close() { assert!(matches!(frontmatter("---\nname: a\ndescription: d\n"), Err(FrontmatterError::Unterminated))); }
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-proto --locked skills::`
Expected: compile error (module missing).

- [ ] **Step 3: Implement `skills.rs`** with the signatures above. `FrontmatterError` derives `Debug, Clone, Copy, PartialEq, Eq`.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo test -p hennery-proto --locked skills::`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-proto/src/skills.rs crates/hennery-proto/src/lib.rs
git commit -m "feat(skills): share skill name and SKILL.md frontmatter rules"
```

---

### Task 2: Wire types, capability and generated schema

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs` (`Capability` L24-65, `HostFrame` L847, `CollectorFrame` L1059, `probe_capability` L966, `mcp_delivery` L998, `probe_request_id` L1040)
- Modify: `crates/hennery-proto/src/rest.rs` (REST types, near `SetHatLogoRequest` L787)
- Modify: `crates/hennery-proto/src/codegen.rs` (both `add!` lists, `render_schema` ~L30 and `render_ts` ~L231)
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Modify: `crates/hennery-sessions/src/ws.rs` L329-566 (the exhaustive `HostFrame` match: add arms that call into Task 4's handlers; for this task a `todo!()`-free placeholder is `tracing::debug!` and `Ok(())`)
- Modify: `crates/hennery-host/src/connection.rs` `handle` L815 (same, a logging arm for the two new `CollectorFrame` variants, replaced in Task 7)

**Interfaces:**
- Produces, in `frames.rs`:
  - `Capability::Skills` (doc: "Skill delivery: `SkillSync`/`SkillArchive` in, `SkillFetch`/`SkillState` out (skills spec §6)"), appended after `PromptMailboxSnapshot`.
  - `pub struct SkillSyncItem { pub id: String, pub name: String, pub content_hash: String }`
  - `CollectorFrame::SkillSync { revision: u64, items: Vec<SkillSyncItem> }`
  - `CollectorFrame::SkillArchive { id: String, content_hash: String, data: String }` (`data` is standard base64 of the tar.gz)
  - `HostFrame::SkillFetch { id: String, content_hash: String }`
  - `pub enum SkillFailure { Conflict, AgentDirMissing, Io, Invalid }` (`snake_case`)
  - `pub struct SkillFailed { pub id: String, pub reason: SkillFailure, pub agent: Option<String> }` (`agent` names `claude` or `codex` when the failure is per agent directory)
  - `HostFrame::SkillState { revision: u64, applied: Vec<String>, failed: Vec<SkillFailed> }`
  - `probe_capability`: the two new collector frames return `Err(NotAProbe)`; `mcp_delivery`: `None`; `probe_request_id`: `None`.
- Produces, in `rest.rs` (all derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS`, `deny_unknown_fields` on requests):
  - `pub struct SkillItem { pub id: String, pub name: String, pub description: String, pub content_hash: String, pub host_ids: Vec<String>, pub created_at: i64, pub updated_at: i64 }`
  - `pub struct SkillDetail { pub item: SkillItem, pub skill_md: String }`
  - `pub struct CreateSkillRequest { pub data: String }` (doc: `POST /api/skills`, base64 tar.gz, step-up; codes `invalid_body`, `invalid_skill`, `skill_too_large`, `skill_exists`, `too_many_skills`)
  - `pub struct ReplaceSkillArchiveRequest { pub data: String }`
  - `pub struct SetSkillHostsRequest { pub host_ids: Vec<String> }`
  - `pub enum SkillHostStatus { Applied, Pending, Failed, Unsupported, Unknown }` (`snake_case`)
  - `pub struct SkillHostState { pub host_id: String, pub status: SkillHostStatus, pub failure: Option<SkillFailed>, pub reported_at: Option<i64> }`
  - `pub struct SkillHostsResponse { pub skill_id: String, pub hosts: Vec<SkillHostState> }`
  - `pub struct HatHostsResponse { pub host_ids: Vec<String> }` (doc: `GET /api/skills/hat-hosts/{hat_id}`)

- [ ] **Step 1: Write the failing test** in `crates/hennery-proto/tests/codegen.rs` style, a new test in `frames.rs` tests:

```rust
#[test] fn skill_frames_round_trip_and_are_not_probes() {
    let sync = CollectorFrame::SkillSync { revision: 3, items: vec![SkillSyncItem { id: "s1".into(), name: "pdf".into(), content_hash: "ab".into() }] };
    let json = serde_json::to_string(&sync).unwrap();
    assert!(json.contains("\"type\":\"skill_sync\""));
    assert_eq!(serde_json::from_str::<CollectorFrame>(&json).unwrap(), sync);
    assert!(matches!(sync.probe_capability(), Err(NotAProbe)));
    let state = HostFrame::SkillState { revision: 3, applied: vec!["s1".into()], failed: vec![SkillFailed { id: "s2".into(), reason: SkillFailure::Conflict, agent: Some("claude".into()) }] };
    assert!(serde_json::to_string(&state).unwrap().contains("\"reason\":\"conflict\""));
    assert_eq!(state.probe_request_id(), None);
    assert!(serde_json::from_str::<Capabilities>("[\"skills\",\"nope\"]").unwrap().has(Capability::Skills));
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `nix develop -c cargo test -p hennery-proto --locked skill_frames`
Expected: compile error.

- [ ] **Step 3: Add the types and variants**, update the three exhaustive matches, add logging arms in `ws.rs` and `connection.rs` so the workspace compiles, register every new `rest` and `frames` type in both `codegen.rs` lists, then regenerate:

Run: `nix develop -c cargo run -p hennery-proto --bin gen --locked`

- [ ] **Step 4: Run the tests and the generator check**

Run: `nix develop -c cargo test -p hennery-proto --locked && nix develop -c cargo run -p hennery-proto --bin gen --locked -- --check && nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: all pass; `generated_schema_matches_the_checked_in_copy` and `generated_ts_matches_the_checked_in_copy` pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-proto schema web/src/generated/protocol.ts crates/hennery-sessions/src/ws.rs crates/hennery-host/src/connection.rs
git commit -m "feat(skills): add skill sync frames, capability and REST types"
```

---

### Task 3: Kernel storage: migration, archive inspection, store methods

**Files:**
- Modify: `crates/hennery-kernel/Cargo.toml` (add `tar.workspace = true`, `flate2.workspace = true`)
- Modify: `crates/hennery-kernel/src/schema.rs` (append migration 17 before `];` at L284)
- Modify: `crates/hennery-kernel/src/session_defaults.rs:161` (the fixture that migrates to `len()-1` now needs `len()-2`, or better: find the migration by content as other tests do)
- Create: `crates/hennery-kernel/src/skill_archive.rs` (inspection)
- Create: `crates/hennery-kernel/src/skills.rs` (`impl Hosts` block with the store methods; `Hosts` is the owner-bound kernel store, `hosts.rs:254`)
- Modify: `crates/hennery-kernel/src/lib.rs` (`pub mod skill_archive; pub mod skills;`)
- Test: in-module tests in both new files

**Interfaces:**
- Consumes: `hennery_proto::skills::{valid_name, frontmatter, Frontmatter}` (Task 1); `hennery_proto::frames::{SkillSyncItem, SkillFailed}` (Task 2).
- Produces, `skill_archive.rs`:
  - `pub const MAX_COMPRESSED: usize = 8 << 20; pub const MAX_UNCOMPRESSED: u64 = 8 << 20; pub const MAX_ENTRIES: usize = 512; pub const MAX_FILE: u64 = 2 << 20;`
  - `pub struct Inspected { pub frontmatter: Frontmatter, pub skill_md: String, pub content_hash: String }` (`content_hash` = lowercase hex sha256 of the compressed bytes)
  - `pub enum Refusal { NotBase64, TooLarge, NotTarGz, Entry(String), NoSkillMd, Frontmatter(FrontmatterError) }`
  - `pub fn from_upload(data: &str) -> Result<(Vec<u8>, Inspected), Refusal>` — length check before decoding (`data.len() > MAX_COMPRESSED.div_ceil(3) * 4` → `TooLarge`), strict `STANDARD` base64 decode, then `inspect`.
  - `pub fn inspect(tgz: &[u8]) -> Result<Inspected, Refusal>` — walks the archive once, never writes to disk: refuses non-file/non-dir entries, paths that are absolute, contain `..`, are not UTF-8, or start with `./` after one optional leading `./`; counts entries (> `MAX_ENTRIES` → `Entry`), bytes (> `MAX_UNCOMPRESSED` → `TooLarge`), per-file size (> `MAX_FILE` → `TooLarge`); reads `SKILL.md` at the root (exactly `SKILL.md`, not nested) into `skill_md`; `NoSkillMd` if absent; parses the frontmatter.
- Produces, `skills.rs` (`impl Hosts`):
  - `pub struct SkillRecord { pub id: String, pub name: String, pub description: String, pub content_hash: String, pub created_at: i64, pub updated_at: i64 }`
  - `pub enum SkillWrite { Exists, TooMany, NameMismatch, Unknown }` (an error enum for refusals; IO/SQL errors are `anyhow`)
  - `pub fn create_skill(&self, inspected: &Inspected, archive: &[u8], now: i64) -> Result<Result<SkillRecord, SkillWrite>>` — `id` is a fresh random 16-byte hex like other ids in the kernel; `Exists` on name clash, `TooMany` at 256.
  - `pub fn replace_skill_archive(&self, id: &str, inspected: &Inspected, archive: &[u8], now: i64) -> Result<Result<SkillRecord, SkillWrite>>` — `NameMismatch` if the new `SKILL.md` name differs.
  - `pub fn delete_skill(&self, id: &str) -> Result<bool>`
  - `pub fn list_skills(&self) -> Result<Vec<SkillRecord>>`, `pub fn skill(&self, id: &str) -> Result<Option<SkillRecord>>`, `pub fn skill_md(&self, id: &str) -> Result<Option<String>>`, `pub fn skill_archive(&self, id: &str) -> Result<Option<(String, Vec<u8>)>>` (hash, bytes)
  - `pub fn set_skill_hosts(&self, id: &str, host_ids: &[String], now: i64) -> Result<Result<(), SkillWrite>>` — replaces the set; unknown or revoked host → `Unknown`.
  - `pub fn skill_hosts(&self, id: &str) -> Result<Vec<String>>`
  - `pub fn skills_revision(&self) -> Result<u64>` — one row per owner in `skill_revisions`, bumped by every write above (create, replace, delete, set hosts).
  - `pub fn skill_sync_for_host(&self, host_id: &str) -> Result<(u64, Vec<SkillSyncItem>)>`
  - `pub struct SkillHostReport { pub revision: u64, pub applied: Vec<String>, pub failed: Vec<SkillFailed>, pub reported_at: i64 }`
  - `pub fn record_skill_state(&self, host_id: &str, report: &SkillHostReport) -> Result<()>` and `pub fn skill_state(&self, host_id: &str) -> Result<Option<SkillHostReport>>` (stored as JSON text columns).

Migration 17 (SQL, in `schema.rs` with a comment "Skills catalogue, phase 1 (skills spec §5)"):

```sql
CREATE TABLE skill_items (
    id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES owners(id),
    name TEXT NOT NULL, description TEXT NOT NULL, skill_md TEXT NOT NULL,
    content_hash TEXT NOT NULL, archive BLOB NOT NULL,
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
    UNIQUE (owner_id, name));
CREATE TABLE skill_assignments (
    owner_id TEXT NOT NULL, skill_id TEXT NOT NULL, host_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (owner_id, skill_id, host_id),
    FOREIGN KEY (skill_id) REFERENCES skill_items(id) ON DELETE CASCADE,
    FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id));
CREATE TABLE skill_host_states (
    owner_id TEXT NOT NULL, host_id TEXT NOT NULL, revision INTEGER NOT NULL,
    applied TEXT NOT NULL, failed TEXT NOT NULL, reported_at INTEGER NOT NULL,
    PRIMARY KEY (owner_id, host_id),
    FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id));
CREATE TABLE skill_revisions (owner_id TEXT PRIMARY KEY REFERENCES owners(id), revision INTEGER NOT NULL);
```

- [ ] **Step 1: Write the failing tests.** In `skill_archive.rs` tests, build archives in memory with `tar::Builder` over `flate2::write::GzEncoder` (helper `fn tgz(entries: &[(&str, &[u8])]) -> Vec<u8>`; a directory entry is a name ending in `/`):

```rust
#[test] fn a_valid_skill_is_inspected() {
    let bytes = tgz(&[("SKILL.md", b"---\nname: pdf\ndescription: d\n---\n# PDF"), ("scripts/", b""), ("scripts/x.py", b"print()")]);
    let got = inspect(&bytes).unwrap();
    assert_eq!(got.frontmatter.name, "pdf"); assert!(got.skill_md.starts_with("---")); assert_eq!(got.content_hash.len(), 64);
}
#[test] fn a_path_outside_the_root_is_refused() {
    for bad in ["../x", "/etc/x", "a/../../x"] {
        let bytes = tgz(&[("SKILL.md", b"---\nname: a\ndescription: d\n---"), (bad, b"x")]);
        assert!(matches!(inspect(&bytes), Err(Refusal::Entry(_))), "{bad}");
    }
}
#[test] fn links_and_special_entries_are_refused()  // build a symlink header with tar::Header::new_gnu + set_entry_type(Symlink); expect Refusal::Entry
#[test] fn limits_are_enforced() // 513 files → Entry; one 2 MiB + 1 byte file → TooLarge; total > 8 MiB over several files → TooLarge
#[test] fn skill_md_must_be_at_the_root() // only "sub/SKILL.md" → NoSkillMd
#[test] fn from_upload_refuses_oversized_and_bad_base64() // "!!!" → NotBase64; a string longer than the cap → TooLarge before decoding
```

In `skills.rs` tests (open `Hosts::open_in_memory()`, register a host with the existing helper used by `hosts.rs` tests, e.g. `register`/`pair` as the file does):

```rust
#[test] fn create_list_replace_delete_bump_the_revision() // revision 0 → 1 after create, 2 after replace, 3 after set_hosts, 4 after delete; list order by name
#[test] fn a_duplicate_name_is_refused() // second create with same name → Ok(Err(SkillWrite::Exists))
#[test] fn the_owner_cap_is_256()
#[test] fn replacing_with_another_name_is_refused() // NameMismatch
#[test] fn sync_for_a_host_lists_only_its_assignments() // two skills, one assigned; skill_sync_for_host returns (revision, [that one]) with name and hash
#[test] fn an_unknown_or_revoked_host_cannot_be_assigned() // SkillWrite::Unknown
#[test] fn host_state_is_recorded_and_read_back()
#[test] fn deleting_a_skill_drops_its_assignments() // cascade
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-kernel --locked skill`
Expected: compile errors.

- [ ] **Step 3: Implement** the migration, `skill_archive.rs` and `skills.rs`. Ids: use the kernel's existing random id helper (grep `fn new_id` / `random_id` in `hosts.rs` or `db.rs`). Revision bump: `INSERT INTO skill_revisions … ON CONFLICT DO UPDATE SET revision = revision + 1 RETURNING revision`, in the same transaction as the write.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo test -p hennery-kernel --locked`
Expected: all pass, including `session_defaults` and `schema` tests (fix the `len()-1` fixture if it broke).

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-kernel
git commit -m "feat(skills): store skills, assignments and host state in the kernel"
```

---

### Task 4: Collector push and host replies

**Files:**
- Create: `crates/hennery-sessions/src/skills_sync.rs`
- Modify: `crates/hennery-sessions/src/lib.rs` (`mod skills_sync;`)
- Modify: `crates/hennery-sessions/src/hub.rs` `HostConn::takes` L176-215
- Modify: `crates/hennery-sessions/src/ws.rs` `ready` L615 (after `resend_idle_holds`), and the `HostFrame` match L329-566 (replace Task 2's logging arms)
- Modify: `crates/hennery-sessions/src/store.rs` (one query, see Interfaces)
- Test: `crates/hennery-sessions/src/skills_sync.rs` tests (hub-level) and `crates/hennery-testkit/tests/skills_sync.rs` (integration, Task 7 extends it)

**Interfaces:**
- Consumes: `Hosts::skill_sync_for_host`, `skill_archive`, `record_skill_state` (Task 3); `Hub::notify(host_id, frame) -> bool`, `Hub::connected_hosts()`, `Hub::has_capability` (hub.rs L457, L505).
- Produces, `skills_sync.rs`:
  - `pub(crate) fn resend_skills(hosts: &Hosts, hub: &Hub, host_id: &str) -> anyhow::Result<()>` — builds `SkillSync` from `skill_sync_for_host` and `notify`s; a host without `Capability::Skills` is skipped (the `takes` gate also refuses it).
  - `pub(crate) fn broadcast_skills(hosts: &Hosts, hub: &Hub) -> anyhow::Result<()>` — `resend_skills` for every `connected_hosts()`; called by the API after every write (Task 5).
  - `pub(crate) fn on_fetch(hosts: &Hosts, hub: &Hub, host_id: &str, id: &str, content_hash: &str) -> anyhow::Result<()>` — answers `SkillArchive` with the stored archive when the hash matches the current one; a stale or unknown hash answers nothing except `tracing::info!` (the host will get a fresh `SkillSync` on the next change).
  - `pub(crate) fn on_state(hosts: &Hosts, host_id: &str, revision: u64, applied: Vec<String>, failed: Vec<SkillFailed>, now: i64) -> anyhow::Result<()>` — `record_skill_state`.
- Produces, `store.rs`: `pub fn hosts_of_hat(&self, hat_id: &str) -> Result<Vec<String>>` running the query: hosts of the owner, not revoked, whose `default_hat_id` is the hat or that have a non-deleted session in it (`sessions.hat_id`, `lifecycle <> 'deleted'`).
- `takes`: `SkillSync` and `SkillArchive` require `Capability::Skills`.

- [ ] **Step 1: Write the failing tests** (hub-level, in `skills_sync.rs` tests, using `Hub` test constructors as `idle_parking.rs` tests do):

```rust
#[test] fn an_old_host_takes_no_skill_sync() // a HostConn registered without Capability::Skills: takes(SkillSync{..}) == false; with it: true
#[test] fn a_fetch_with_the_current_hash_is_answered_with_the_archive() // hub records the notified frame; assert SkillArchive{data} decodes to the stored bytes
#[test] fn a_fetch_with_a_stale_hash_is_ignored()
#[test] fn a_state_report_is_stored()
#[test] fn hosts_of_hat_unions_default_hat_and_sessions() // in store.rs tests: host A default hat X, host B ran a session in X, host C neither → [A, B]
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-sessions --locked skills_sync hosts_of_hat`

- [ ] **Step 3: Implement**, wire `ready()` to call `skills_sync::resend_skills(&state.hosts, &state.hub, host_id)?` after the idle resends, and route `HostFrame::SkillFetch`/`SkillState` in `ws.rs` to `on_fetch`/`on_state`. `SkillArchive` is built with `base64::engine::general_purpose::STANDARD.encode`.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo test -p hennery-sessions --locked`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-sessions
git commit -m "feat(skills): push skill sync to hosts and record their state"
```

---

### Task 5: `/api/skills` router and the `skills` feature flag

**Files:**
- Create: `crates/hennery-sessions/src/skills_api.rs`
- Modify: `crates/hennery-sessions/src/lib.rs` `router` L209 (`.merge(skills_api::router(state.clone()))`)
- Modify: `crates/hennery-kernel/src/capabilities.rs:27` (`"skills"` appended) and its exact-match test at `:77`; `crates/hennery-testkit/tests/capabilities.rs:41`
- Test: `crates/hennery-testkit/tests/skills_api.rs` (HTTP, like `logos.rs`: `operator_client`, `owner_phc`)

**Interfaces:**
- Consumes: Task 3 store methods; Task 4 `broadcast_skills`; `hennery_kernel::json::ApiJson`; `api::error`, `api::internal` (`crates/hennery-sessions/src/api.rs:181,208`); `hennery_kernel::auth::{operator_only, require_step_up}`.
- Produces routes (JSON, `Cache-Control: private, no-store` like `session_defaults_api.rs`):
  - `GET /api/skills` → `Vec<SkillItem>`
  - `POST /api/skills` (step-up, `DefaultBodyLimit::max(SKILL_BODY_LIMIT)` where `SKILL_BODY_LIMIT = MAX_COMPRESSED.div_ceil(3) * 4 + 1024`) → 201 `SkillItem`; errors 400 `invalid_skill` (any `Refusal` except size, message names the entry or frontmatter error), 413 `skill_too_large`, 409 `skill_exists`, 409 `too_many_skills`
  - `GET /api/skills/{id}` → `SkillDetail`; 404 `skill_not_found`
  - `PUT /api/skills/{id}/archive` (step-up, same limit) → `SkillItem`; 409 `skill_name_mismatch`
  - `DELETE /api/skills/{id}` (step-up) → 204
  - `PUT /api/skills/{id}/hosts` (step-up, 16 KiB) body `SetSkillHostsRequest` → `SkillHostsResponse`; 400 `unknown_host`
  - `GET /api/skills/{id}/hosts` → `SkillHostsResponse` — status per assigned host, from the host's latest `SkillHostReport` (a report covers the host's whole set): `unsupported` when the host's persisted `capabilities` (`HostRecord.capabilities`) lack `skills`; `unknown` when no report exists; `applied` when the latest report lists the id in `applied`; `failed` with the `SkillFailed` when listed in `failed`; `pending` otherwise (assigned after the last report).
  - `GET /api/skills/hat-hosts/{hat_id}` → `HatHostsResponse` from `Store::hosts_of_hat`
  - Every write ends with `skills_sync::broadcast_skills(&state.hosts, &state.hub)`.
  - Decoding and inspection run in `tokio::task::spawn_blocking` (the logo handler's pattern, `hats.rs:195`).

- [ ] **Step 1: Write the failing HTTP tests** in `crates/hennery-testkit/tests/skills_api.rs`, with a `tgz` helper duplicated from Task 3 (or exported from `hennery_kernel::skill_archive::test_support` behind `cfg(any(test, feature = "test-hooks"))`, whichever `logos.rs` precedent fits):

```rust
#[tokio::test] async fn a_skill_is_created_listed_detailed_and_deleted_behind_step_up()
// stale session POST → 403 step_up_required; fresh POST → 201 with name "pdf"; GET list has one; GET {id} has skill_md; DELETE → 204; list empty
#[tokio::test] async fn a_duplicate_name_is_refused() // second POST → 409 skill_exists
#[tokio::test] async fn an_invalid_archive_names_its_problem() // archive without SKILL.md → 400 invalid_skill, message contains "SKILL.md"
#[tokio::test] async fn an_oversized_upload_is_refused_before_decoding() // body with data of MAX_COMPRESSED*2 base64 chars → 413 (skill_too_large or body_too_large, assert status only)
#[tokio::test] async fn hosts_are_assigned_and_reported() // pair host-a (no report) → status unknown; PUT hosts [host-a] → response has host-a unknown; GET hosts → same
#[tokio::test] async fn hat_hosts_lists_default_hat_hosts() // host-a default hat X → GET hat-hosts/X == [host-a]
#[tokio::test] async fn capabilities_advertise_skills() // GET /api/capabilities features contains "skills"
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-testkit --locked --test skills_api`

- [ ] **Step 3: Implement `skills_api.rs`**, add the feature, fix the two exact-match capability tests.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo test -p hennery-testkit --locked --test skills_api --test capabilities && nix develop -c cargo test -p hennery-kernel --locked capabilities`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-sessions crates/hennery-kernel/src/capabilities.rs crates/hennery-testkit/tests
git commit -m "feat(skills): add the /api/skills router and feature flag"
```

---

### Task 6: Host store: extract, link, reconcile

**Files:**
- Create: `crates/hennery-host/src/skills.rs`
- Modify: `crates/hennery-host/src/lib.rs` (`pub mod skills;`)
- Modify: `crates/hennery-host/src/runtime/extract.rs` (a second entry point without the first-component strip; see Interfaces)
- Test: in-module tests with `tempfile::tempdir()`

**Interfaces:**
- Consumes: `agent_home::resolve(agent, env) -> Option<AgentHome>` (`agent_home.rs:48`, `AgentHome.root`), `agent_home::{CLAUDE, CODEX}`; `hennery_proto::frames::{SkillSyncItem, SkillFailed, SkillFailure}`; `hennery_proto::skills::{valid_name, frontmatter}`; `runtime::install::swap_link` pattern (L895; make a shared `pub(crate) fn replace_symlink(link: &Path, target: &Path) -> io::Result<()>` in `extract.rs` or a new `fsutil` if cleaner: temp link beside, then `rename`).
- Produces, `extract.rs`: `pub fn tree(tgz: &[u8], dest: &Path, limits: Limits) -> Result<()>` with `pub struct Limits { pub max_bytes: u64, pub max_entries: usize, pub max_file: u64 }`; same refusals as `package` but no strip, reads from a byte slice, keeps the executable bit only for paths under `scripts/`, writes everything else `0644`, directories `0755`.
- Produces, `skills.rs`:
  - `pub const DIR: &str = "skills"; pub const FILE: &str = "skills.db";`
  - `pub struct Store { root: PathBuf, registry: Mutex<Connection> }` — `pub fn open(data_dir: &Path) -> Result<Self>` creates `<data_dir>/skills/` (0700) and the registry table `skills(id TEXT PRIMARY KEY, name TEXT NOT NULL, content_hash TEXT NOT NULL, dir TEXT NOT NULL)` and `links(id TEXT NOT NULL, agent TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(id, agent))`.
  - `pub struct AgentDirs { pub claude: Option<PathBuf>, pub codex: Option<PathBuf> }` and `pub fn agent_dirs(env: &[(String, String)]) -> AgentDirs` — `resolve(CLAUDE, env)` / `resolve(CODEX, env)` roots joined with `skills`; `None` when the agent root does not exist.
  - `pub enum Need { Have, Fetch }` and `pub fn plan(&self, items: &[SkillSyncItem]) -> Vec<(SkillSyncItem, Need)>` — `Have` when the registry has the id with the same hash.
  - `pub fn install(&self, item: &SkillSyncItem, tgz: &[u8], dirs: &AgentDirs) -> Vec<SkillFailed>` — verify sha256 of `tgz` equals `item.content_hash` (else `Invalid`); verify `valid_name(item.name)`; extract with `tree` into `<root>/<id>.<hash[..12]>/<name>/` (a fresh directory; `Invalid` on refusal, `Io` on error); check `SKILL.md` frontmatter name equals `item.name` (`Invalid`); then for each agent dir: `link(dirs.claude, "claude")`, `link(dirs.codex, "codex")`; on success record `skills` and `links` rows and remove the previous `<id>.<oldhash>` directory if any.
  - `fn link(&self, agent_dir: Option<&Path>, agent: &str, item, target: &Path) -> Option<SkillFailed>` — `None` dir → `AgentDirMissing{agent}`; create `agent_dir` (the `skills` subdirectory) with `create_dir_all` if its parent exists; if `agent_dir/<name>` exists and is not a symlink whose target starts with `self.root` → `Conflict{agent}`; else `replace_symlink`.
  - `pub fn remove(&self, id: &str) -> Vec<SkillFailed>` — for every `links` row whose path is still a symlink into `self.root`: unlink; a path that is now something else is left and reported as `Conflict`; then remove the store directory (`std::fs::remove_dir_all` on a path hennery created, under `self.root`, no walk needed) and the rows.
  - `pub fn reconcile(&self, items: &[SkillSyncItem], dirs: &AgentDirs) -> (Vec<String> /* applied ids */, Vec<SkillFailed>, Vec<SkillSyncItem> /* to fetch */)` — removes registry entries not in `items`, re-links `Have` entries whose link is missing (dangling or absent) and reports conflicts, returns the `Fetch` list.

- [ ] **Step 1: Write the failing tests** (each builds a `tempdir` with `data/`, `home/.claude/`, `home/.codex/`, and an env `[("HOME", home)]`; archives via a local `tgz` helper):

```rust
#[test] fn install_extracts_and_links_into_both_agents() // links exist, resolve to SKILL.md; registry has rows; scripts/x.py is executable, SKILL.md is 0644
#[test] fn a_missing_agent_dir_is_reported_and_the_other_still_links() // no home/.codex → failed [AgentDirMissing{codex}], claude link present
#[test] fn a_foreign_path_is_a_conflict_and_survives_removal() // pre-create home/.claude/skills/pdf/ as a real dir → Conflict{claude}; remove(id) leaves it
#[test] fn a_hash_mismatch_or_bad_name_is_invalid()
#[test] fn extraction_refuses_an_escaping_path() // tree() with "../x" entry → Err; dest has nothing outside it
#[test] fn a_changed_hash_replaces_the_directory_and_switches_the_link() // install v1, install v2 same id: link target changes, old dir gone
#[test] fn reconcile_drops_unlisted_skills_and_relinks_missing_links() // delete the claude link by hand → reconcile recreates; unlisted id → removed, link gone
#[test] fn remove_only_touches_links_into_the_store()
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-host --locked skills::`

- [ ] **Step 3: Implement** `tree`, `replace_symlink`, `skills.rs`.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo test -p hennery-host --locked`
Expected: pass (including the existing `runtime` tests of `package`).

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-host
git commit -m "feat(host): materialise skills into the agents' skill directories"
```

---

### Task 7: Host connection: capability, sync handling, state reply

**Files:**
- Modify: `crates/hennery-host/src/connection.rs` — capability list in `handshake` L397-415 (`Capability::Skills`), `run_until` L171-184 (open `skills::Store` beside `homes`, thread it through `connect_once` L452 and `handle` L815), the `SkillSync`/`SkillArchive` arms
- Test: `crates/hennery-testkit/tests/skills_host.rs` (fake collector, the `host_connection.rs` helpers: `accept_host`, `send_frame`, `read_until`, `unique_data_dir`)

**Interfaces:**
- Consumes: Task 6 `skills::Store`; `Uplink::reply(HostFrame)` (unsequenced, best-effort).
- Produces behaviour:
  - On `SkillSync { revision, items }`: in `std::thread::spawn` (the `handle` function is synchronous; follow `forget()` L1156): `reconcile(items, agent_dirs(process env))`; for each item to fetch, `uplink.reply(HostFrame::SkillFetch{..})`; remember `revision` and the pending fetch set in `Store` (an in-memory `Mutex<Pending { revision, waiting: BTreeSet<String>, applied, failed }>`); if nothing is pending, reply `SkillState` at once.
  - On `SkillArchive { id, content_hash, data }`: decode base64 (`Invalid` on failure), `install`, move the id from `waiting` to `applied` or `failed`; when `waiting` is empty, reply `SkillState { revision, applied, failed }` with the full set (applied includes `Have` items).
  - A `SkillSync` with a new revision while fetches are pending replaces the pending set.
  - `HostConfig` (connection.rs L46) gains `pub agent_env: Vec<(String, String)>`, set to `std::env::vars().collect()` by `HostConfig::new`; `agent_dirs(&cfg.agent_env)` is computed once in `run_until`. Tests overwrite the field with `HOME` pointing at a temp home, so no test touches the process environment.

- [ ] **Step 1: Write the failing integration tests** in `skills_host.rs` (build the `HostConfig` as `forget_host.rs` does, then set `cfg.agent_env = vec![("HOME".into(), temp_home)]` with `temp_home/.claude` and `temp_home/.codex` created):

```rust
#[tokio::test] async fn hello_announces_skills_and_a_sync_with_nothing_answers_state() // Hello.capabilities has Skills; send SkillSync{revision:1, items:[]} → SkillState{revision:1, applied:[], failed:[]}
#[tokio::test] async fn a_sync_fetches_installs_and_reports() // SkillSync with one item → SkillFetch{id,hash}; send SkillArchive → SkillState{applied:[id]}; both symlinks exist
#[tokio::test] async fn a_changed_skill_is_refetched_and_relinked() // second sync with new hash → fetch again → link target changed
#[tokio::test] async fn a_bad_archive_is_reported_invalid_and_nothing_is_linked() // data not base64 → failed [Invalid]
#[tokio::test] async fn a_reconnect_resyncs_from_the_registry() // restart host (new run with same data_dir), sync with same item → no SkillFetch, SkillState applied at once
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop -c cargo test -p hennery-testkit --locked --test skills_host`

- [ ] **Step 3: Implement** the connection changes.

- [ ] **Step 4: Run the tests**

Run: `nix develop -c cargo test -p hennery-testkit --locked --test skills_host --test host_connection --test forget_host`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-host crates/hennery-testkit
git commit -m "feat(host): announce and apply skill sync from the collector"
```

---

### Task 8: Web: `/skills` screen

**Files:**
- Create: `web/src/api/skills.ts`, `web/src/screens/Skills.tsx`, `web/src/screens/Skills.test.tsx`, `web/src/components/SkillHosts.tsx`
- Modify: `web/src/routes.ts` (`'skills'` in `RouteName`, `'/skills': 'skills'` in `STATIC`), `web/src/lib/views.ts` (`View`, `BY_MODE.full`, `LABEL.skills = 'Skills'`, `PATH.skills = '/skills'`), `web/src/screens/More.tsx` (add `'skills'` to the list), `web/src/components/Shell.tsx` (gate on `caps.features.includes('skills')` in `advertised` L44, navigation L57 and the route render L67-72 with a `Placeholder` fallback), `web/src/api/errors.ts` `MESSAGES` (new codes), `web/src/App.test.tsx:171-174` (More links now include `/skills` when the feature is on; keep the existing assertion for a capabilities fixture without it)
- Test fixtures: `web/src/test-fixtures.ts` gets `skill(over)`.

**Interfaces:**
- Consumes: generated `SkillItem`, `SkillDetail`, `SkillHostsResponse`, `SkillHostState`, `HatHostsResponse`, `CreateSkillRequest`, `SetSkillHostsRequest` from `web/src/generated/protocol.ts`; `Client.request`; `fileToBase64` (`web/src/lib/image.ts:12`); `Markdown`; `useResource`; `hosts(client)` and `hats(client)` from `api/manage.ts`; `Text`/`visible`.
- Produces, `api/skills.ts`: `listSkills(client, signal?)`, `skill(client, id)`, `createSkill(client, data: string)`, `replaceSkillArchive(client, id, data)`, `deleteSkill(client, id)`, `skillHosts(client, id)`, `setSkillHosts(client, id, hostIds: string[])`, `hatHosts(client, hatId)`.
- Produces, `SkillHosts.tsx`: `export default function SkillHosts({ skillId, hosts, hats, onChanged }: { skillId: string; hosts: HostItem[] | null; hats: HatItem[] | null; onChanged?(): void })` — a `<fieldset className="mounts">` of host checkboxes (the `Mounts.tsx` pattern: save the whole set on each tick, ignore ticks while a save runs), each row showing the host's status word: `applied`, `pending`, `failed: <reason>[ (<agent>)]`, `unsupported` ("host too old"), `unknown`; above it a `<select>` of hats with a "Select this hat's hosts" button that calls `hatHosts` and ticks those hosts (union with the current set).
- Produces, `Skills.tsx`: `export default function Skills()` — `manage` layout like `Mcp.tsx`: heading "Skills", subtitle "Skill directories your hosts link for Claude Code and Codex.", an "Upload skill" button opening a hidden `<input type="file" accept=".tar.gz,.tgz">` (Composer's pattern) → `fileToBase64` → `createSkill`; a card per skill (name, description, hash prefix 12 chars, "Hosts" count); selecting a card shows the detail: `Markdown` of `skill_md`, `SkillHosts`, "Replace archive" and "Delete" (with `ConfirmDialog`). Errors through `messageOf`.

- [ ] **Step 1: Write the failing tests** in `Skills.test.tsx` (the `Mcp.test.tsx` harness: `open(routes, capabilities)` with `history.replaceState(null, '', '/skills')`, `stubServer`, `render(<App fetchImpl={server.fetch}/>)`, `stepUp()`):

```ts
it('lists skills and shows a detail with its SKILL.md', …)   // GET /api/skills → [skill()], GET /api/skills/s1 → detail; click card; expect heading text from markdown
it('uploads a tar.gz behind step-up', …)                     // fire change on the file input with a File; first POST → STEP_UP, then 201; expect server.sent to have POST /api/skills with body.data a base64 string
it('ticks hosts and saves the whole set', …)                  // PUT /api/skills/s1/hosts body { host_ids: ['host-a'] }
it('a_host_without_the_capability_is_marked', …)              // GET hosts state with status 'unsupported' → row text contains 'host too old'
it('select this hat’s hosts ticks the hat’s hosts', …)        // GET /api/skills/hat-hosts/hat-1 → ['host-b']; click; PUT with ['host-b']
it('is hidden without the feature', …)                        // capabilities without 'skills' → Placeholder text, no GET /api/skills
```

- [ ] **Step 2: Run them to see them fail**

Run: `nix develop .#web -c pnpm --dir web test -- Skills`

- [ ] **Step 3: Implement** the API module, components, routing and gating; add `skill_exists: 'A skill with that name already exists.'`, `invalid_skill: 'The archive is not a valid skill.'`, `skill_too_large: 'The skill archive is too large.'`, `too_many_skills: 'The skill limit is reached.'`, `skill_name_mismatch: 'The new archive has a different skill name.'`, `skill_not_found: 'That skill no longer exists.'`, `unknown_host: 'One of the hosts is unknown or revoked.'` to `MESSAGES`.

- [ ] **Step 4: Run the web gates**

Run: `nix develop .#web -c pnpm --dir web typecheck && nix develop .#web -c pnpm --dir web test`
Expected: pass, including `App.test.tsx`.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): add the Skills screen with host assignment"
```

---

### Task 9: Docs, changelog and the full gate run

**Files:**
- Modify: `README.md` "What works" list (one bullet: "**Skills.** Upload Agent Skills directories once and choose the hosts that link them into `~/.claude/skills` and `~/.codex/skills`."), `CHANGELOG.md` (Unreleased: Added), `docs/README.md` (plan entry under Plans pointing at this file, status line), `docs/specs/2026-11-17-skills-hooks-catalogue.md` (a "Phase 1 built" note under §12 with the names actually used: `skill_*`, `Skill*`, `Capability::Skills`, `/api/skills`, BLOB storage instead of a file store, one frame per archive)
- Modify: `nix/package.nix:19-53` only if a new `.sql`/`.json`/fixture file was added (Task 3 keeps SQL inline in `schema.rs`, so normally nothing)

- [ ] **Step 1: Update the documents**

- [ ] **Step 2: Run every Rust gate**

```bash
nix develop -c cargo fmt --all --check
nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
nix develop -c cargo clippy -p hennery --locked -- -D warnings
nix develop -c cargo test --workspace --locked
nix develop -c cargo run -p hennery-proto --bin gen --locked -- --check
nix develop -c cargo run -p hennery-view --bin gen-view --locked -- --check
```
Expected: all pass. (Per the user's memory, `reverse_proxy` and `join` tests need the Nix shell; they are included above.)

- [ ] **Step 3: Run the web gates and the Nix source audit**

```bash
nix develop .#web -c pnpm --dir web typecheck && nix develop .#web -c pnpm --dir web test && nix develop .#web -c pnpm --dir web build
nix build -L .#checks.x86_64-linux.source-inputs
```
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add README.md CHANGELOG.md docs
git commit -m "docs(skills): record phase 1 of the skills catalogue"
```

---

## Self-review notes

- Spec coverage, phase 1: §5 model and limits → Tasks 1, 3, 5; §6 transfer → Tasks 2, 4, 7 (one frame per archive instead of chunks, within `MAX_FRAME`); §7 materialisation → Task 6; §9 API and screen → Tasks 5, 8; §10 restart/reconcile → Tasks 6, 7 (`reconcile`, `a_reconnect_resyncs_from_the_registry`); §10 adapter probes and §8 hooks → phase 2 plan. Archive storage as a BLOB in the kernel instead of a file beside `attachments/` is a builder choice the spec allows (§11); it avoids the orphan sweep and a second file store.
- Names are consistent across tasks: `SkillSyncItem`, `SkillFailed`, `SkillFailure`, `Capability::Skills`, `Hosts::skill_sync_for_host`, `skills::Store::{plan, install, remove, reconcile}`, `skills_sync::{resend_skills, broadcast_skills, on_fetch, on_state}`.
- Review Focus items 1–5 each have a named test in Tasks 3, 5, 6, 7, 4/8.
