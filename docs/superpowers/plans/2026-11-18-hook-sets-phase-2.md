# Hook sets, phase 2 (per-session hook delivery) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The operator keeps named hook sets (Claude Code `settings.json`-shaped `hooks` JSON) in the catalogue, assigns them to hosts, and every session started by hennery on such a host receives them: Claude through the adapter's programmatic settings, Codex through `CODEX_CONFIG` plus the adapter's `_codex/hooks/trust`, fail-closed.

**Architecture:** The kernel stores hook sets (typed JSON, validated) and assignments. The collector attaches a `hooks` delivery to `StartSession`/`ResumeSession`/`DelegatedAttach` for hosts that announce `Capability::HookSets`, and refuses to send to hosts that do not. The host renders the merged sets per agent profile, trusts them in Codex between `initialize` and `session/new`, fails the start when it cannot, and acknowledges what ran in `Indexed`. A "Hook sets" tab joins the Skills screen; the session details panel names the applied sets.

**Tech Stack:** Rust (axum, rusqlite, serde, agent-client-protocol 5.2 `UntypedMessage`), React + TypeScript (vitest).

**Spec:** `docs/specs/2026-11-17-skills-hooks-catalogue.md` §8 (hooks at session start), §9 (API and screen), §10 (adapter probes); phase 1 is in `docs/superpowers/plans/2026-11-17-skills-catalogue-phase-1.md` and its names are reused.

## Global Constraints

- Naming: `hook_set`/`hook_sets` (tables `hook_sets`, `hook_set_assignments`; `Capability::HookSets`; `/api/hook-sets`; feature flag `hook_sets`; frames field `hooks`). Never `catalog`.
- A hook set is a name (skill-name rules: `hennery_proto::skills::valid_name`) plus a typed `hooks` object: event → matcher groups → handlers; handlers are `type: "command"` only; every other handler type and every unknown field is refused (`deny_unknown_fields`). Limits: JSON body ≤ 64 KiB; ≤ 32 events; ≤ 16 groups per event; ≤ 8 handlers per group; `matcher` ≤ 256 chars; `command` 1–4096 chars; `timeout` 1–600 seconds when present; ≤ 64 hook sets per owner. Events must be Claude Code event names (the list in Task 1).
- Phase 2 carries **no bundled files** (spec §5's optional `files` archive and `${HENNERY_HOOK_DIR}` are deferred to a later phase); commands must name programs that already exist on the host, for example a script shipped inside a skill at `~/.claude/skills/<name>/scripts/…`.
- Rendering: Claude gets every event; Codex gets only `SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`, `PermissionRequest`, `PreCompact`, `PostCompact`, `SubagentStart`, `SubagentStop`, in that spelling.
- Delivery is fail-closed: a host without `Capability::HookSets` that has hook sets assigned never receives the start (collector refusal `hook_sets_unsupported`); a host profile that cannot apply them (`Generic`) refuses before spawning (`HostFrame::Error` code `hook_sets_unsupported`); a Codex trust that does not return `trusted: true`, or an adapter without the extension, fails the start with `StartFailed` code `hook_sets_refused`; a hooks change for a live actor is refused like a source-policy change (`hook_sets_conflict`).
- hennery never edits the operator's files for hooks; the only persistent side effect is Codex's `hooks.state` written by the adapter's trust call.
- Every write route behind `hennery_kernel::auth::require_step_up`; every query owner-scoped; `crates/hennery-kernel/src/hook_sets.rs` registered in `crates/hennery-testkit/tests/owner_filter.rs` SOURCES with its exact statement count.
- Untrusted strings in the UI through `Text`/`visible`.
- Toolchain on this machine: no `nix`; `env PATH=/home/abankowski/toolchain-dl/rust-official/bin:$PATH` for fmt/clippy; always `CARGO_INCREMENTAL=0`; host/testkit tests under `umask 022`; known environmental failures (reverse_proxy-dependent targets, `hennery` `deployment`/`readme`) are not findings; clippy allows `-A clippy::nonminimal_bool -A clippy::search_is_some` for two pre-existing lints. Web: `npx --yes pnpm@10 …` from `web/`. Check `df` before a workspace test build; delete only this worktree's `target/`.
- Commit style: Conventional Commits with scope `hooks`, `host`, `sessions`, `web`; trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

## Review Focus

1. A hook set whose JSON carries a non-command handler (`type: "prompt"`), an unknown key, or an unknown event must be refused at upload with a message naming the problem, never stored. Test: Task 2 (`a_non_command_handler_is_refused`, `an_unknown_event_is_refused`).
2. A Codex session must not start with untrusted hooks: when `_codex/hooks/trust` answers `trusted: false` or the method is missing, the start fails with `hook_sets_refused` and the adapter process is stopped. Test: Task 4 (`a_codex_trust_refusal_fails_the_start`, `an_adapter_without_the_hooks_extension_fails_the_start`).
3. The operator's own Claude hooks keep running: the host must only add `options.settings.hooks` and keep `extraArgs`/`strictMcpConfig` intact in `_meta`. Test: Task 4 (`claude_meta_keeps_strict_mcp_and_adds_hooks`).
4. An operator-configured `CODEX_CONFIG` on the agent command must be merged, not clobbered, and a non-object value refused. Test: Task 4 (`an_operator_codex_config_is_merged_with_the_hooks`).
5. An older host (no `HookSets` capability) with hook sets assigned must get a visible refusal, not a silent start without hooks. Test: Task 3 (`a_host_without_the_capability_is_refused_the_start`), Task 6 (`a_host_without_the_capability_is_marked`).

---

### Task 1: Hook-set model, wire and REST types in `hennery-proto`

**Files:**
- Create: `crates/hennery-proto/src/hooks.rs`
- Modify: `crates/hennery-proto/src/lib.rs` (`pub mod hooks;`), `crates/hennery-proto/src/frames.rs` (`Capability`, the three attach frames, `Indexed`, `CollectorFrame::hooks_delivery`), `crates/hennery-proto/src/rest.rs` (REST types, `SessionDetail`), `crates/hennery-proto/src/codegen.rs` (both lists)
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`

**Interfaces:**
- Produces, `hooks.rs` (all derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS`):
  - `pub const CLAUDE_EVENTS: &[&str]` = `SessionStart, Setup, SessionEnd, UserPromptSubmit, UserPromptExpansion, Stop, StopFailure, PreToolUse, PermissionRequest, PermissionDenied, PostToolUse, PostToolUseFailure, PostToolBatch, SubagentStart, SubagentStop, TaskCreated, TaskCompleted, TeammateIdle, ConfigChange, InstructionsLoaded, CwdChanged, DirectoryAdded, FileChanged, WorktreeCreate, WorktreeRemove, PreCompact, PostCompact, PreModelSwitch, PostModelSwitch, Notification, MessageDisplay, Elicitation, ElicitationResult`
  - `pub const CODEX_EVENTS: &[&str]` = the nine in Global Constraints.
  - `pub struct CommandHook { #[serde(rename = "type")] pub kind: HookKind, pub command: String, #[serde(default, skip_serializing_if = "Option::is_none")] pub timeout: Option<u32> }` with `pub enum HookKind { #[serde(rename = "command")] Command }` (so any other `type` fails deserialisation); `#[serde(deny_unknown_fields)]`.
  - `pub struct MatcherGroup { #[serde(default, skip_serializing_if = "Option::is_none")] pub matcher: Option<String>, pub hooks: Vec<CommandHook> }`, `deny_unknown_fields`.
  - `pub struct HookSetConfig(pub BTreeMap<String, Vec<MatcherGroup>>)` (transparent map event → groups).
  - `pub enum HookSetError { TooManyEvents, UnknownEvent(String), TooManyGroups(String), TooManyHandlers(String), MatcherTooLong(String), CommandEmpty(String), CommandTooLong(String), TimeoutOutOfRange(String) }` and `pub fn validate(config: &HookSetConfig) -> Result<(), HookSetError>` with the limits above.
  - `pub fn render(config: &HookSetConfig, events: &[&str]) -> HookSetConfig` — keeps only the listed events (used with `CODEX_EVENTS`; Claude passes everything).
  - `pub fn merge<'a>(sets: impl IntoIterator<Item = &'a HookSetConfig>) -> HookSetConfig` — event by event, groups appended in input order.
  - `pub fn events_of(config: &HookSetConfig) -> Vec<String>` (sorted keys).
- Produces, `frames.rs`:
  - `Capability::HookSets` (doc: "Per-session hook delivery: `hooks` on start/resume/delegated attach, acknowledged in `Indexed.hook_sets_applied` (skills spec §8)"), appended after `Skills`.
  - `pub struct HookSetDelivery { pub id: String, pub name: String, pub content_hash: String, pub hooks: HookSetConfig }`, `pub struct HookDelivery { pub sets: Vec<HookSetDelivery> }`.
  - On `StartSession`, `ResumeSession`, `DelegatedAttach`: `#[serde(default, skip_serializing_if = "Option::is_none")] #[ts(optional)] pub hooks: Option<HookDelivery>` (not flattened).
  - `pub fn hooks_delivery(&self) -> Option<&HookDelivery>` on `CollectorFrame`, exhaustive like `mcp_delivery` (L1082).
  - `pub struct HookSetApplied { pub id: String, pub name: String, pub content_hash: String }`; `Indexed.hook_sets_applied: Vec<HookSetApplied>` with `#[serde(default, skip_serializing_if = "Vec::is_empty")]` (doc: host-owned acknowledgement, only on `SessionStarted`).
- Produces, `rest.rs`:
  - `HookSetItem { id, name, content_hash, events: Vec<String>, host_ids: Vec<String>, created_at: i64, updated_at: i64 }`, `HookSetDetail { item: HookSetItem, hooks: HookSetConfig }`, `CreateHookSetRequest { name: String, hooks: HookSetConfig }`, `ReplaceHookSetRequest { hooks: HookSetConfig }`, `SetHookSetHostsRequest { host_ids: Vec<String> }` (requests `deny_unknown_fields`), `HookSetHostStatus { Assigned, Unsupported, Offline }` (snake_case), `HookSetHostState { host_id, status }`, `HookSetHostsResponse { hook_set_id, hosts }`, `HookEventSupport { event: String, claude: bool, codex: bool }`, `HookEventsResponse { events: Vec<HookEventSupport> }`.
  - `SessionDetail` gains `#[serde(default, skip_serializing_if = "Vec::is_empty")] #[ts(optional)] pub hook_sets: Vec<HookSetApplied>`.
  - Doc comments list routes and codes: `invalid_hook_set` (400, message names the `HookSetError`), `hook_set_exists` (409), `too_many_hook_sets` (409), `hook_set_not_found` (404), `unknown_host` (400).

- [ ] **Step 1: Write the failing tests** in `hooks.rs` `#[cfg(test)]`:

```rust
#[test] fn a_command_hook_set_parses_and_validates() { let c: HookSetConfig = serde_json::from_str(r#"{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/x.sh","timeout":10}]}]}"#).unwrap(); assert!(validate(&c).is_ok()); assert_eq!(events_of(&c), vec!["PreToolUse"]); }
#[test] fn other_handler_types_and_unknown_fields_do_not_parse() { for bad in [r#"{"Stop":[{"hooks":[{"type":"prompt","prompt":"x"}]}]}"#, r#"{"Stop":[{"hooks":[{"type":"command","command":"x","if":"Bash(*)"}]}]}"#] { assert!(serde_json::from_str::<HookSetConfig>(bad).is_err(), "{bad}"); } }
#[test] fn validation_enforces_the_limits() // unknown event → UnknownEvent; 33 events → TooManyEvents; 17 groups → TooManyGroups; 9 handlers → TooManyHandlers; matcher 257 → MatcherTooLong; empty command → CommandEmpty; 4097 → CommandTooLong; timeout 0 and 601 → TimeoutOutOfRange
#[test] fn render_keeps_only_codex_events_and_merge_appends_groups() // {Stop, PreToolUse} rendered with CODEX_EVENTS → {PreToolUse}; merge of two sets both with PreToolUse → 2 groups in order
```

In `crates/hennery-proto/tests/frames.rs`: `hook_delivery_is_optional_on_the_wire_and_acknowledged` — a `StartSession` without `hooks` serialises without the key and deserialises with `None`; with `Some(HookDelivery{..})` round-trips; `hooks_delivery()` returns it for the three frames and `None` for `SkillSync`; `Indexed` default omits `hook_sets_applied`; `Capabilities` from `["hook_sets"]` has `HookSets`.

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-proto --locked hooks:: hook_delivery`
Expected: compile errors.

- [ ] **Step 3: Implement**, register every new type in both `codegen.rs` lists (frames types in `render_ts`, REST types in both), regenerate with `cargo run -p hennery-proto --bin gen --locked`, and make the workspace compile (the three frame builders in `crates/hennery-sessions` get `hooks: None` for now; `crates/hennery-host/src/connection.rs` destructures `hooks: _`).

- [ ] **Step 4: Run the tests and the generator check**

Run: `cargo test -p hennery-proto --locked && cargo run -p hennery-proto --bin gen --locked -- --check && cargo check --workspace --locked`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-proto schema web/src/generated/protocol.ts crates/hennery-sessions crates/hennery-host
git commit -m "feat(hooks): add hook-set model, delivery frames and REST types"
```

---

### Task 2: Kernel storage for hook sets

**Files:**
- Modify: `crates/hennery-kernel/src/schema.rs` (append migration 18)
- Create: `crates/hennery-kernel/src/hook_sets.rs` (`impl Hosts` block, like `skills.rs`)
- Modify: `crates/hennery-kernel/src/lib.rs` (`pub mod hook_sets;`), `crates/hennery-kernel/src/hosts.rs` `try_revoke` (~L663: also `hook_sets::drop_host_assignments(tx, owner, host_id)`)
- Modify: `crates/hennery-testkit/tests/owner_filter.rs` (SOURCES entry for `hennery-kernel/src/hook_sets.rs` with its exact statement count; bump `hosts.rs` floor if its count changes)

**Interfaces:**
- Consumes: `hennery_proto::hooks::{HookSetConfig, validate, events_of}`, `hennery_proto::frames::HookSetDelivery`, `hennery_proto::skills::valid_name`.
- Produces, `hook_sets.rs`:
  - `pub const MAX_HOOK_SETS: i64 = 64;`
  - `pub struct HookSetRecord { pub id: String, pub name: String, pub content_hash: String, pub events: Vec<String>, pub created_at: i64, pub updated_at: i64 }`
  - `pub enum HookSetWrite { Exists, TooMany, Invalid(HookSetError), BadName, Unknown }`
  - `pub fn create_hook_set(&self, name: &str, hooks: &HookSetConfig, now: i64) -> Result<Result<HookSetRecord, HookSetWrite>>` — validates name and config, `content_hash` = sha256 hex of the canonical JSON (`serde_json::to_vec` of the `HookSetConfig`, whose `BTreeMap` makes it canonical).
  - `pub fn replace_hook_set(&self, id: &str, hooks: &HookSetConfig, now: i64) -> Result<Result<HookSetRecord, HookSetWrite>>`
  - `pub fn delete_hook_set(&self, id: &str) -> Result<bool>` (cascade to assignments)
  - `pub fn list_hook_sets(&self) -> Result<Vec<HookSetRecord>>`, `pub fn hook_set(&self, id: &str) -> Result<Option<(HookSetRecord, HookSetConfig)>>`
  - `pub fn set_hook_set_hosts(&self, id: &str, host_ids: &[String], now: i64) -> Result<Result<(), HookSetWrite>>` (`Unknown` for an unknown or revoked host), `pub fn hook_set_hosts(&self, id: &str) -> Result<Vec<String>>`
  - `pub fn hook_sets_for_host(&self, host_id: &str) -> Result<Vec<HookSetDelivery>>` ordered by name.
  - `pub(crate) fn drop_host_assignments(tx: &Transaction, owner: &str, host_id: &str) -> Result<()>`.

Migration 18:

```sql
CREATE TABLE hook_sets (
    id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES owners(id),
    name TEXT NOT NULL, hooks TEXT NOT NULL, content_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
    UNIQUE (owner_id, name));
CREATE TABLE hook_set_assignments (
    owner_id TEXT NOT NULL, hook_set_id TEXT NOT NULL, host_id TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (owner_id, hook_set_id, host_id),
    FOREIGN KEY (hook_set_id) REFERENCES hook_sets(id) ON DELETE CASCADE,
    FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id));
```

- [ ] **Step 1: Write the failing tests** in `hook_sets.rs` (open `Hosts::open_in_memory()`, register hosts as `skills.rs` tests do):

```rust
#[test] fn create_list_replace_delete_round_trip() // record.events == ["PreToolUse"]; replace changes content_hash; delete cascades assignments
#[test] fn a_duplicate_name_is_refused() // Exists
#[test] fn a_non_command_handler_is_refused() // create with a config parsed leniently? No: the typed config cannot hold it, so this test lives at the API layer (Task 3); here: an unknown event → Invalid(UnknownEvent)
#[test] fn an_unknown_event_is_refused()
#[test] fn the_owner_cap_is_64()
#[test] fn hosts_are_assigned_and_delivered_in_name_order() // two sets assigned to host-a → hook_sets_for_host("host-a") names in order with hooks
#[test] fn an_unknown_or_revoked_host_cannot_be_assigned()
#[test] fn revoking_a_host_drops_its_hook_set_assignments() // via Hosts::revoke
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-kernel --locked hook_set`

- [ ] **Step 3: Implement** migration, store, revoke hook, owner-filter registration.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p hennery-kernel --locked && umask 022 && cargo test -p hennery-testkit --locked --test owner_filter`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-kernel crates/hennery-testkit/tests/owner_filter.rs
git commit -m "feat(hooks): store hook sets and their host assignments in the kernel"
```

---

### Task 3: Collector: `/api/hook-sets`, delivery assembly, capability refusal, acknowledgement

**Files:**
- Create: `crates/hennery-sessions/src/hook_sets_api.rs`
- Modify: `crates/hennery-sessions/src/lib.rs` (`mod hook_sets_api;` and `.merge(hook_sets_api::router(state.clone()))`), `crates/hennery-kernel/src/capabilities.rs` (`"hook_sets"` feature; fix its two exact-match tests in kernel and `crates/hennery-testkit/tests/capabilities.rs`)
- Modify: `crates/hennery-sessions/src/api.rs` (`start_in_hat` ~L501 frame, `resume_authorized` ~L1032 frame; a new `pub(crate) fn hook_delivery(state: &AppState, host_id: &str) -> anyhow::Result<Option<HookDelivery>>` returning `None` when no sets are assigned; `request_failed_for_session`, `resume_failed`, `not_sent_reason` for the new error), `crates/hennery-sessions/src/hub.rs` (`RequestError::HookSetsUnsupported`; `takes` refuses a start-type frame whose `hooks_delivery().is_some()` when the host lacks `Capability::HookSets`; `Hub::wait` ~L656 maps that refusal to the new variant rather than the MCP ones), `crates/hennery-sessions/src/delegation_worker.rs` ~L295 (compute `hook_delivery` and pass it), `crates/hennery-sessions/src/delegation_dispatch.rs` `attach_delegated(..)` and `delegation_delivery_dispatch.rs` `attach_notice_parent(..)` (new `hooks: Option<HookDelivery>` parameter placed in the frame), `crates/hennery-sessions/src/store.rs` (sessions migration: `ALTER TABLE sessions ADD COLUMN hook_sets_applied TEXT NOT NULL DEFAULT '[]'`; on `SessionStarted` store `indexed.hook_sets_applied` as JSON; `session_detail` reads it into `SessionDetail.hook_sets`), `web/src/components/items/Marker.tsx` `REASON.start_failed` (`hook_sets_unsupported: 'the host cannot apply the assigned hook sets'`, `hook_sets_refused: 'the agent did not accept the assigned hook sets'`, `hook_sets_conflict: 'the running agent has different hook sets'`), `web/src/api/errors.ts` `MESSAGES` (`hook_set_exists`, `too_many_hook_sets`, `hook_set_not_found`, `hook_sets_unsupported`)
- Test: `crates/hennery-testkit/tests/hook_sets_api.rs` (HTTP, model `skills_api.rs`; the fake host over the websocket announces capabilities so the start frame can be observed)

**Interfaces:**
- Consumes: Task 2 store methods; Task 1 types; `skills_api.rs` helpers pattern (`not_found`, `invalid`, `written`), `api::error`/`internal`.
- Produces routes (JSON, `Cache-Control: private, no-store`, body limit 64 KiB + 4 KiB for the two JSON write routes, 16 KiB for hosts):
  - `GET /api/hook-sets` → `Vec<HookSetItem>`; `POST /api/hook-sets` (step-up) → 201 `HookSetItem`; 400 `invalid_hook_set` (an `ApiJson` deserialisation failure of the typed config is also answered as 400 `invalid_hook_set` with the serde message, so a `type: "prompt"` handler names itself); 409 `hook_set_exists`; 409 `too_many_hook_sets`
  - `GET /api/hook-sets/events` → `HookEventsResponse` (every `CLAUDE_EVENTS` entry, `codex` true when in `CODEX_EVENTS`)
  - `GET /api/hook-sets/{id}` → `HookSetDetail`; `PUT /api/hook-sets/{id}` (step-up) body `ReplaceHookSetRequest` → `HookSetItem`; `DELETE` (step-up) → 204
  - `GET|PUT /api/hook-sets/{id}/hosts` → `HookSetHostsResponse`; status: `unsupported` when the host's stored capabilities lack `hook_sets` or the live connection lacks it, `offline` when no ready connection, else `assigned`.
  - No broadcast: hook sets take effect at the next session start.
- `hook_delivery` is called in `start_in_hat` before the frame is built, in `resume_authorized`, and in `delegation_worker` for both delegated attaches.
- A refused start (host lacks the capability) marks the session failed with reason `hook_sets_unsupported` (409 to the API caller, like `mcp_isolation_unavailable`).

- [ ] **Step 1: Write the failing HTTP tests**:

```rust
#[tokio::test] async fn a_hook_set_is_created_listed_detailed_and_deleted_behind_step_up()
#[tokio::test] async fn a_non_command_handler_is_refused() // POST with {"type":"prompt"} → 400 invalid_hook_set, message mentions "command"
#[tokio::test] async fn an_unknown_event_is_refused() // 400 invalid_hook_set mentioning the event
#[tokio::test] async fn events_report_per_agent_support() // GET events: PreToolUse claude+codex, UserPromptSubmit claude only
#[tokio::test] async fn a_start_carries_the_assigned_hook_sets() // fake host with capabilities [.., hook_sets] connected; assign set to it; POST /api/sessions start → the StartSession frame read from the fake host's socket has hooks.sets[0].name
#[tokio::test] async fn a_host_without_the_capability_is_refused_the_start() // fake host without hook_sets; assign; start → 409 hook_sets_unsupported; session failure_reason == "hook_sets_unsupported"; no StartSession frame
#[tokio::test] async fn a_session_started_ack_records_the_applied_sets() // fake host answers SessionStarted with indexed.hook_sets_applied → GET /api/sessions/{id} detail hook_sets == [...]
#[tokio::test] async fn capabilities_advertise_hook_sets()
```

- [ ] **Step 2: Run them to see them fail**

Run: `umask 022 && cargo test -p hennery-testkit --locked --test hook_sets_api`

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run the tests**

Run: `umask 022 && cargo test -p hennery-sessions --locked && cargo test -p hennery-testkit --locked --test hook_sets_api --test capabilities --test skills_api && cargo test -p hennery-kernel --locked capabilities`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-sessions crates/hennery-kernel/src/capabilities.rs crates/hennery-testkit/tests web/src/components/items/Marker.tsx web/src/api/errors.ts
git commit -m "feat(hooks): deliver assigned hook sets at session start and expose /api/hook-sets"
```

---

### Task 4: Host: render, inject, trust, acknowledge

**Files:**
- Modify: `crates/hennery-host/src/profile.rs` (`pub fn supports_hook_sets(self) -> bool`: `Claude | ClaudeOwnCli | Codex` true, `Generic` false; `pub fn session_meta_with_hooks(self, source: Option<&McpSourceSnapshot>, hooks: Option<&HookSetConfig>) -> Option<Map<String, Value>>` that takes `session_meta_for`'s object and, for Claude profiles with hooks, inserts `claudeCode.options.settings = {"hooks": <config>}` keeping every existing key)
- Create: `crates/hennery-host/src/hook_sets.rs`: `pub struct Rendered { pub claude_meta_hooks: Option<HookSetConfig>, pub codex_config: Option<String> /* JSON for CODEX_CONFIG */, pub applied: Vec<HookSetApplied> }`, `pub fn render(profile: Profile, delivery: &HookDelivery, existing_codex_config: Option<&str>) -> Result<Rendered, HookSetsError>` (merge sets; Claude: full merge; Codex: `render(.., CODEX_EVENTS)` then `{"hooks": …}` merged into the existing `CODEX_CONFIG` object when present — `HookSetsError::CodexConfigNotObject` when it is not a JSON object), `pub async fn trust_codex(conn: &ConnectionTo<Agent>, cwd: &Path) -> Result<(), HookSetsError>` (`_codex/hooks/list` → collect `{key, currentHash}` of every entry with `source == "sessionFlags"`; `_codex/hooks/trust`; `trusted != true` → `HookSetsError::NotTrusted`; JSON-RPC method-not-found → `HookSetsError::NoExtension`)
- Modify: `crates/hennery-host/src/session.rs` (`Launch.hooks: Option<HookDelivery>`; in `drive`: render before `agent_env` is cloned (~L1297), push `("CODEX_CONFIG", json)` for Codex, use `session_meta_with_hooks` in `SessionMcp.meta`; store `hook_sets_applied` on the actor; `negotiate` gains `codex_trust: bool` and, right after `initialize` (~L2637), calls `trust_codex` when set, mapping errors to `StartError { code: "hook_sets_refused", message }`; `announce` fills `indexed.hook_sets_applied`; `probe_catalog` passes no hooks)
- Modify: `crates/hennery-host/src/connection.rs` (`AttachRequest.hooks: Option<HookDelivery>`; `attach`: refuse with `HostFrame::Error { code: "hook_sets_unsupported" }` when `req.hooks.is_some() && !profile.supports_hook_sets()`; `spawn_or_restart`: a live actor whose launch hooks differ (compare `content_hash` lists) → `HostFrame::Error { code: "hook_sets_conflict" }`; the three frame arms pass `hooks`; Hello capabilities gain `Capability::HookSets`)
- Modify: `crates/hennery-testkit/src/lib.rs` + `src/bin/hennery-fake-acp.rs` (`FakeScript.codex_hooks: Option<FakeCodexHooks { list: Vec<FakeHookEntry { key: String, current_hash: String }>, trusted: bool }>`; when set, the fake answers `_codex/hooks/list` with those entries (`source: "sessionFlags"`, `trustStatus: "untrusted"`) and `_codex/hooks/trust` with `{trusted}`; both calls and the `CODEX_CONFIG` env value are appended to `session_log` as `{"method": "_codex/hooks/list"|"_codex/hooks/trust", "params": ..}` and `{"env": {"CODEX_CONFIG": ..}}` lines)
- Test: `crates/hennery-testkit/tests/hook_sets_host.rs` (host-level, `session_mcp.rs`'s `attach()` harness building `Launch` directly, plus frame-level cases via `host_connection.rs` helpers)

**Interfaces:**
- Consumes: Task 1 types; `UntypedMessage::new(method, params)` + `conn.send_request(..).block_task().await` (session.rs L23, L1034 pattern).
- Produces behaviour listed above; `Indexed.hook_sets_applied` = every delivered set (Claude) or every delivered set that had at least one Codex-supported event (Codex), in delivery order.

- [ ] **Step 1: Write the failing tests** (fixtures: a `HookDelivery` with two sets, one `PreToolUse` command hook and one `UserPromptSubmit` command hook):

```rust
#[tokio::test] async fn claude_meta_keeps_strict_mcp_and_adds_hooks() // Profile::Claude: logged session/new _meta has claudeCode.options.extraArgs["strict-mcp-config"] AND claudeCode.options.settings.hooks.PreToolUse; SessionStarted.indexed.hook_sets_applied has both sets
#[tokio::test] async fn codex_gets_filtered_config_and_is_trusted() // Profile::Codex, fake codex_hooks list of one entry, trusted: true → logged env CODEX_CONFIG parses to {"hooks": {PreToolUse: ..}} without UserPromptSubmit; log shows list then trust with the entry; hook_sets_applied has only the PreToolUse set
#[tokio::test] async fn an_operator_codex_config_is_merged_with_the_hooks() // AgentCommand env CODEX_CONFIG={"model":"x"} → logged value has both model and hooks; a CODEX_CONFIG of "[]" → StartFailed hook_sets_refused before any ACP call
#[tokio::test] async fn a_codex_trust_refusal_fails_the_start() // trusted: false → StartFailed code hook_sets_refused; adapter process exited (pid_alive false)
#[tokio::test] async fn an_adapter_without_the_hooks_extension_fails_the_start() // codex_hooks None → method_not_found → hook_sets_refused
#[tokio::test] async fn a_generic_profile_refuses_before_spawning() // Profile::Generic with hooks → HostFrame::Error hook_sets_unsupported, no process
#[tokio::test] async fn no_hooks_means_no_settings_and_no_trust_calls() // hooks None → _meta without settings, no _codex/hooks lines, CODEX_CONFIG absent
#[tokio::test] async fn hello_announces_hook_sets() // capability list (update the exact-list test in host_connection.rs)
```

- [ ] **Step 2: Run them to see them fail**

Run: `umask 022 && cargo test -p hennery-testkit --locked --test hook_sets_host`

- [ ] **Step 3: Implement** (profile, `hook_sets.rs`, session, connection, fake adapter); update the pinned-meta tests (`profile.rs:313`, `tests/session_mcp.rs`, `tests/mcp_source_host.rs`, `tests/host_connection.rs:1641`) only if their expectations change — they should not, since without hooks the meta is unchanged.

- [ ] **Step 4: Run the tests**

Run: `umask 022 && cargo test -p hennery-host --locked && cargo test -p hennery-testkit --locked --test hook_sets_host --test session_mcp --test mcp_source_host --test host_connection`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-host crates/hennery-testkit
git commit -m "feat(host): inject hook sets per session and trust them in Codex"
```

---

### Task 5: Web: Hook sets tab, session details line

**Files:**
- Create: `web/src/api/hookSets.ts` (`listHookSets`, `hookSet(id)`, `createHookSet(name, hooks)`, `replaceHookSet(id, hooks)`, `deleteHookSet(id)`, `hookSetHosts(id)`, `setHookSetHosts(id, hostIds)`, `hookEvents()`), `web/src/components/HookSetEditor.tsx`, `web/src/components/HookSetHosts.tsx`, `web/src/screens/HookSets.test.tsx`
- Modify: `web/src/screens/Skills.tsx` (ARIA tabs "Skills" | "Hook sets" following `screens/Defaults.tsx:78-88`; the Hook sets panel renders when `caps.features` includes `hook_sets`), `web/src/components/SkillHosts.tsx` (generalise: props `load(): Promise<{hosts: {host_id, status, failure?}[]}>`, `save(ids): Promise<same>`, `legend: string`, `statusWord(state)`; `SkillHosts` and `HookSetHosts` become thin wrappers), `web/src/components/SessionDetails.tsx` (a "Hook sets" row listing `detail.hook_sets` names through `Text`, after the MCP delivery block), `web/src/api/errors.ts` (`invalid_hook_set` is NOT mapped so the server message shows), `web/src/test-fixtures.ts` (`hookSet(over)`)

**Interfaces:**
- `HookSetEditor`: `{ initial?: HookSetConfig; events: HookEventSupport[]; onSubmit(name: string, hooks: HookSetConfig): Promise<void>; busy: boolean }` — a name input (hidden on replace), a `<textarea>` with the JSON, client-side `JSON.parse` with an inline error, and a table of events present in the JSON with Claude/Codex marks ("Claude only" for events Codex ignores).
- Hook sets panel: list cards (name, events count, hosts count), detail with the editor (replace), `HookSetHosts`, Delete behind `ConfirmDialog`; "New hook set" opens the editor empty.

- [ ] **Step 1: Write the failing tests** (`HookSets.test.tsx`, harness as `Skills.test.tsx` with `features: ['skills', 'hook_sets']`):

```ts
it('shows the Hook sets tab only with the feature', …)          // without 'hook_sets' no tab; with it, click tab → GET /api/hook-sets
it('creates a hook set behind step-up and shows Codex support marks', …) // type name + JSON with PreToolUse and UserPromptSubmit; submit → STEP_UP then 201; marks: UserPromptSubmit "Claude only"
it('shows the server reason for an invalid hook set', …)          // POST → 400 invalid_hook_set "handler type must be command" → text visible
it('ticks hosts and saves the set', …)                            // PUT /api/hook-sets/h1/hosts body { host_ids: ['host-a'] }
it('a_host_without_the_capability_is_marked', …)                  // status unsupported → "host too old"
it('session details list applied hook sets', …)                   // render a session whose detail has hook_sets [{name:'guard'}] → "Hook sets: guard"
```

- [ ] **Step 2: Run them to see them fail**

Run (from `web/`): `npx --yes pnpm@10 test -- HookSets`

- [ ] **Step 3: Implement.**

- [ ] **Step 4: Run the web gates**

Run (from `web/`): `npx --yes pnpm@10 typecheck && npx --yes pnpm@10 test`
Expected: pass, including `Skills.test.tsx` and `Session*.test.tsx`.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): add the Hook sets tab and show applied sets in session details"
```

---

### Task 6: Live adapter probes, docs and the gate run

**Files:**
- Create: `crates/hennery-testkit/tests/hook_sets_live.rs` — two `#[ignore = "requires pinned adapters and credentials"]` tests in the `delegation_live.rs` style (same env vars `HENNERY_D4_PINNED_ADAPTER_ROOT`, `HENNERY_D4_CLAUDE_AUTH`, `HENNERY_D4_CODEX_AUTH`, scratch homes): `claude_runs_delivered_hooks_beside_file_hooks` (a `SessionStart` command hook writing a marker file; a project `.claude/settings.json` hook too; both markers appear) and `codex_trusts_and_runs_delivered_hooks` (`SessionStart` marker; the scratch `CODEX_HOME/config.toml` gains a `hooks.state` entry). The spike scripts in `spikes/2026-11-17-hooks-delivery/` are the reference for prompts and markers.
- Modify: `README.md` ("What works" Skills bullet mentions hook sets), `CHANGELOG.md` (Unreleased entry), `docs/README.md` (plan entry; spec status "phases 1 and 2 built"), `docs/specs/2026-11-17-skills-hooks-catalogue.md` §12 ("Phase 2 built" note: no bundled files, pre-spawn `HostFrame::Error` for unsupported, `StartFailed` for refused, the per-host statuses for hook sets, the live probes' names and how to run them)

- [ ] **Step 1: Write the two ignored live tests** (they must compile and be skipped by default).

- [ ] **Step 2: Update the documents.**

- [ ] **Step 3: Run every gate** (check `df` first; `umask 022`; `CARGO_INCREMENTAL=0`):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings -A clippy::nonminimal_bool -A clippy::search_is_some
cargo test --workspace --locked --no-fail-fast
cargo run -p hennery-proto --bin gen --locked -- --check
cargo run -p hennery-view --bin gen-view --locked -- --check
(cd web && npx --yes pnpm@10 typecheck && npx --yes pnpm@10 test)
```
Expected: only the known environmental targets fail.

- [ ] **Step 4: Commit**

```bash
git add README.md CHANGELOG.md docs crates/hennery-testkit/tests/hook_sets_live.rs
git commit -m "docs(hooks): record phase 2 of the skills catalogue"
```

---

## Self-review notes

- Spec coverage: §8 rendering per agent (T1 `render`, T4), Claude `settings` (T4), Codex `CODEX_CONFIG` + trust (T4), fail-closed start (T3 collector refusal, T4 host refusals), `SessionStarted` acknowledgement and the start card (T1 `Indexed`, T3 store/detail, T5 details line), step-up on writes (T3), host Settings listing of received sets → replaced by the per-host status on the Hook sets tab and the session-details line (T5; builder's call recorded in §12), §9 routes and the tab with per-event support marks (T3, T5), §10 pin-bump probes (T6). Not built: bundled files archive (Global Constraints).
- Names consistent: `HookSetConfig`, `HookDelivery { sets }`, `HookSetDelivery`, `HookSetApplied`, `Capability::HookSets`, `hook_delivery(state, host_id)`, `profile.supports_hook_sets()`, `hook_sets::render/trust_codex`, codes `hook_sets_unsupported` / `hook_sets_refused` / `hook_sets_conflict`, feature `hook_sets`.
- Review Focus 1–5 are pinned in T2/T3, T4, T4, T4, T3/T5.
