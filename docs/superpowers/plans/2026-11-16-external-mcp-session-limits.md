# External MCP Session Limits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hardcoded 32 live-session cap for external MCP clients with a per-client capacity set at consent, add a fixed per-client creation rate fuse, and make both discoverable and documented.

**Architecture:** One new column on `external_clients` carries the capacity; both existing refusal sites read it. A typed `LimitRefusal` error (Display = bare code) lets the existing string-matched error plumbing keep working while the MCP endpoint attaches text and `structuredContent` for the two limit codes. The rate fuse counts recent `session_create`/`session_clone` rows in the `external_operations` ledger inside the admission transaction. Consent UI and REST list carry the capacity; spec and tool descriptions document both limits.

**Tech Stack:** Rust (axum, rusqlite, anyhow, serde_json), React + TypeScript (vitest, testing-library).

**Spec:** `docs/superpowers/specs/2026-11-16-external-mcp-session-limits-design.md`

## Global Constraints

- Capacity default `32`, accepted range `1..=1024`; values outside → `invalid_grant`.
- Rate fuse: `16` creations per rolling `60` s per client; constants only, no config surface.
- `live` = rows of `external_client_sessions` for the client joined to `sessions` with `lifecycle NOT IN ('closed','deleted','failed') AND close_requested = 0`.
- Refusals are tool results with `isError: true` on HTTP 200, never JSON-RPC `error`, never HTTP 429.
- Internal error strings for `ensure!`/`bail!` stay bare codes: `session_limit`, `session_rate_limit`.
- `external_clients.revision` is never bumped by anything in this plan.
- New migration goes at the **end** of `MIGRATIONS` in `crates/hennery-sessions/src/store.rs`.
- Rust gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`. Web gates: `npm run typecheck`, `npx vitest run` in `web/`.
- Commit messages end with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

## Review Focus

1. Consent body carrying `session_capacity` without `control_sessions` is accepted and stored (not rejected as unknown field) — Task 1 test `capacity_is_stored_without_control`.
2. A same-`request_key` retry inside the rate window returns the original receipt and does not count as a new creation — Task 3 test `same_key_retry_does_not_consume_rate_budget`.
3. A session with `close_requested = 1` that the host has not yet confirmed frees capacity immediately — Task 2 test `close_requested_session_frees_capacity`.
4. `session_clone` is subject to both limits exactly like `session_create` — Task 3 test `clone_counts_toward_rate` and Task 2 assertion on clone in `capacity_refuses_at_limit`.
5. The second guard at dispatch (`external_guard_in`) re-checks capacity but never rate, so an admitted creation is not refused later because of the window — Task 3 test `guard_in_does_not_apply_rate`.

---

### Task 1: Capacity column and grant field

**Files:**
- Create: `crates/hennery-sessions/src/external_capacity.sql`
- Modify: `crates/hennery-sessions/src/store.rs:470-482` (MIGRATIONS list, append)
- Modify: `crates/hennery-sessions/src/external_clients.rs:33-67` (`NewClient`, `ClientRecord`), `:118-150` (`record`), `:394-410` (`issue_in` validation), `:471-472` (INSERT)
- Modify: `crates/hennery-sessions/src/external_oauth_store.rs:129-150` (`Consent`), `:348-358` (conversion)
- Modify: every `NewClient { … }` literal: `external_clients.rs` tests, `external_control.rs` tests, `crates/hennery-testkit/tests/external_control.rs` (add `session_capacity: None`)
- Test: `crates/hennery-sessions/src/external_clients.rs` (tests module, `World` harness)

**Interfaces:**
- Produces: `pub const DEFAULT_SESSION_CAPACITY: u32 = 32;` and `pub const MAX_SESSION_CAPACITY: u32 = 1024;` in `external_clients.rs`.
- Produces: `NewClient.session_capacity: Option<u32>` (`#[serde(default)]`, `None` = default), `Consent.session_capacity: Option<u32>` (same), `ClientRecord.session_capacity: u32` (serialised, so REST `GET /api/external-clients` and `POST` responses carry it).

- [ ] **Step 1: Write the failing tests** in the `tests` module of `external_clients.rs`, using `World::new()` and `w.grant(true)`:

```rust
#[test]
fn capacity_defaults_to_32_and_is_bounded() {
    let w = World::new();
    let (c, _) = w.store.issue_external_client(&w.grant(true)).unwrap();
    assert_eq!(c.session_capacity, 32);
    let (c, _) = w.store.issue_external_client(&NewClient { session_capacity: Some(100), control_sessions: true, ..w.grant(true) }).unwrap();
    assert_eq!(c.session_capacity, 100);
    assert_eq!(w.store.external_clients_after("").unwrap().iter().find(|r| r.client_id == c.client_id).unwrap().session_capacity, 100);
    for bad in [0, 1025] {
        let err = w.store.issue_external_client(&NewClient { session_capacity: Some(bad), ..w.grant(true) }).unwrap_err();
        assert_eq!(err.to_string(), "invalid_grant");
    }
}
#[test]
fn capacity_is_stored_without_control() {
    let w = World::new();
    let (c, _) = w.store.issue_external_client(&NewClient { session_capacity: Some(5), control_sessions: false, ..w.grant(true) }).unwrap();
    assert_eq!(c.session_capacity, 5);
}
#[test]
fn rows_without_capacity_read_back_as_32() {
    // Simulates a grant issued before the column existed: the DEFAULT applies.
    let w = World::new();
    w.store.conn().execute("INSERT INTO external_clients(owner_id,id,label,hat_id,token_hash,read_metadata,existing_sessions,revision,expires_at) VALUES(?1,'old','Old',?2,'hash',1,0,1,9999999999)", rusqlite::params![w.store.owner_id(), w.hat]).unwrap();
    assert_eq!(crate::external_clients::record(&w.store.conn(), w.store.owner_id(), "old").unwrap().unwrap().session_capacity, 32);
}
#[test]
fn consent_body_accepts_session_capacity() {
    let c: crate::external_oauth_store::Consent = serde_json::from_value(serde_json::json!({"request_id":"r","allow":true,"session_capacity":7})).unwrap();
    assert_eq!(c.session_capacity, Some(7));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-sessions external_clients::tests::capacity -- --nocapture` and `cargo test -p hennery-sessions external_clients::tests::consent_body`
Expected: compile error (no field `session_capacity`).

- [ ] **Step 3: Add the migration** `external_capacity.sql`:

```sql
ALTER TABLE external_clients ADD COLUMN session_capacity INTEGER NOT NULL DEFAULT 32 CHECK(session_capacity BETWEEN 1 AND 1024);
```

Append `include_str!("external_capacity.sql"),` as the **last** entry of `MIGRATIONS` in `store.rs` (after `work.sql`).

- [ ] **Step 4: Add the fields and constants.** In `external_clients.rs`: the two `pub const`s; `NewClient.session_capacity: Option<u32>` with `#[serde(default)]`; `ClientRecord.session_capacity: u32`; `record()` selects `session_capacity` as a 13th column; `issue_in` validates `input.session_capacity.map_or(true, |c| (1..=MAX_SESSION_CAPACITY).contains(&c))` else `invalid_grant`, and inserts `input.session_capacity.unwrap_or(DEFAULT_SESSION_CAPACITY)` as a new column in the INSERT. In `external_oauth_store.rs`: `Consent.session_capacity: Option<u32>` with `#[serde(default)]`, passed through in the `NewClient` literal. Add `session_capacity: None` to every other `NewClient` literal so the crate and testkit compile.

- [ ] **Step 5: Run the tests and the crate's existing suites**

Run: `cargo test -p hennery-sessions external_clients` then `cargo test -p hennery-sessions` and `cargo test -p hennery-testkit --test external_control`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hennery-sessions/src/external_capacity.sql crates/hennery-sessions/src/store.rs crates/hennery-sessions/src/external_clients.rs crates/hennery-sessions/src/external_oauth_store.rs crates/hennery-sessions/src/external_control.rs crates/hennery-testkit/tests/external_control.rs
git commit -m "feat(mcp): store per-client session capacity on external grants"
```

---

### Task 2: Capacity enforcement and typed limit refusals

**Files:**
- Modify: `crates/hennery-sessions/src/external_control_store.rs:218-225` (`external_guard_in` create branch), `:309-319` (`external_admit` create/clone branch)
- Modify: `crates/hennery-sessions/src/external_mcp.rs:352-379` (generic error branch)
- Test: `crates/hennery-sessions/src/external_control.rs` tests module (`world()` harness); `crates/hennery/tests/external_mcp.rs` (`Collector` harness)

**Interfaces:**
- Consumes: `ClientRecord.session_capacity` from Task 1.
- Produces in `external_control_store.rs`:
  ```rust
  #[derive(Debug)]
  pub struct LimitRefusal { pub code: &'static str, pub text: String, pub data: serde_json::Value }
  impl std::fmt::Display for LimitRefusal { /* writes self.code only */ }
  impl std::error::Error for LimitRefusal {}
  pub(crate) fn live_sessions_in(conn: &rusqlite::Connection, owner: &str, client_id: &str) -> anyhow::Result<i64>
  pub(crate) fn capacity_refusal(live: i64, limit: u32) -> LimitRefusal
  ```
  `capacity_refusal` builds `code = "session_limit"`, `text = format!("session_limit: {live} of {limit} live sessions in use; close sessions created by this client to free capacity")`, `data = json!({"code":"session_limit","live":live,"limit":limit})`.
- Produces in `external_mcp.rs`: the generic `Err(e)` branch first does `if let Some(r) = e.downcast_ref::<LimitRefusal>() { return result(id, json!({"content":[{"type":"text","text":r.text}],"structuredContent":r.data,"isError":true})); }`, then the existing `match`.

- [ ] **Step 1: Write the failing unit tests** in `external_control.rs` tests. Helper to add alongside `world()`: `fn world_with_capacity(capacity: u32) -> (TempDir, AppState, ExternalIdentity)` identical to `world()` but `session_capacity: Some(capacity)`. Helper `fn bind(state: &AppState, identity: &ExternalIdentity, key: &str, cwd: &str) -> String` that admits `session_create` with `request_key = key`, takes the pending command via `external_pending()`, calls `external_begin`, then `state.store.create_session_authorized(&receipt.session_id, "host", "fake", cwd, &hat, None, McpContext::NONE, Some(&command))` (pub(crate), `store.rs:2231`; this is the variant that inserts the `external_client_sessions` row), then `external_finish(&command, "completed", None, None)`, returning the session id. `hat` is `state.hosts.host("host").unwrap().unwrap().default_hat_id`.

```rust
#[test]
fn capacity_refuses_at_limit_and_reports_numbers() {
    let (_d, state, identity) = world_with_capacity(2);
    bind(&state, &identity, "a", "/project");
    let parked = bind(&state, &identity, "b", "/project");
    state.store.conn().execute("UPDATE sessions SET lifecycle='parked' WHERE id=?1", [&parked]).unwrap();
    let err = state.store.external_admit(&identity, "session_create", "c", &json!({"request_key":"c","host_id":"host","cwd":"/project","agent":"fake"})).unwrap_err();
    assert_eq!(err.to_string(), "session_limit");
    let r = err.downcast_ref::<crate::external_control_store::LimitRefusal>().unwrap();
    assert_eq!(r.data, json!({"code":"session_limit","live":2,"limit":2}));
    assert_eq!(r.text, "session_limit: 2 of 2 live sessions in use; close sessions created by this client to free capacity");
    let err = state.store.external_admit(&identity, "session_clone", "d", &json!({"request_key":"d","source_session_id":parked})).unwrap_err();
    assert_eq!(err.to_string(), "session_limit");
}
#[test]
fn closed_session_frees_capacity() {
    let (_d, state, identity) = world_with_capacity(1);
    let s = bind(&state, &identity, "a", "/project");
    state.store.close_now(&s).unwrap();
    state.store.external_admit(&identity, "session_create", "b", &json!({"request_key":"b","host_id":"host","cwd":"/project","agent":"fake"})).unwrap();
}
#[test]
fn close_requested_session_frees_capacity() {
    let (_d, state, identity) = world_with_capacity(1);
    let s = bind(&state, &identity, "a", "/project");
    state.store.conn().execute("UPDATE sessions SET lifecycle='active', close_requested=1 WHERE id=?1", [&s]).unwrap();
    state.store.external_admit(&identity, "session_create", "b", &json!({"request_key":"b","host_id":"host","cwd":"/project","agent":"fake"})).unwrap();
}
#[test]
fn guard_in_uses_client_capacity() {
    let (_d, state, identity) = world_with_capacity(1);
    bind(&state, &identity, "a", "/project");
    state.store.conn().execute("UPDATE external_clients SET session_capacity=2", []).unwrap();
    let r = state.store.external_admit(&identity, "session_create", "b", &json!({"request_key":"b","host_id":"host","cwd":"/project","agent":"fake"})).unwrap();
    let (command, _) = state.store.external_pending().unwrap().into_iter().find(|(c,_)| c.operation_id == r.operation_id).unwrap();
    state.store.conn().execute("UPDATE external_clients SET session_capacity=1", []).unwrap();
    let hat = state.hosts.host("host").unwrap().unwrap().default_hat_id;
    let err = state.store.external_guard_in(&state.store.conn(), &command, Some(("host", "/project", &hat))).unwrap_err();
    assert_eq!(err.to_string(), "session_limit");
}
```

- [ ] **Step 2: Write the failing HTTP-shape test** in `crates/hennery/tests/external_mcp.rs`, after `unknown_tools_are_protocol_errors…`:

```rust
#[tokio::test]
async fn limit_refusals_are_tool_results_with_text_and_structured_content() {
    let c = Collector::new().await;
    let mut grant = c.grant(false);
    grant["control_sessions"] = json!(true); grant["session_capacity"] = json!(1);
    let issued: Value = c.operator.post(format!("{}/api/external-clients", c.url)).json(&grant).send().await.unwrap().json().await.unwrap();
    assert_eq!(issued["client"]["session_capacity"], 1);
    let hat = c.state.hosts.host("test-host").unwrap().unwrap().default_hat_id;
    c.state.store.create_session("bound", "test-host", "fake", "/project", &hat, None).unwrap();
    let db = rusqlite::Connection::open(c.dir.path().join("test.db")).unwrap(); // the file Collector::new opens
    db.execute("INSERT INTO external_client_sessions(owner_id,client_id,session_id) SELECT owner_id,id,'bound' FROM external_clients WHERE id=?1", [issued["client"]["client_id"].as_str().unwrap()]).unwrap();
    let body: Value = c.rpc(issued["token"].as_str().unwrap(), "tools/call", json!({"name":"session_create","arguments":{"request_key":"k","host_id":"test-host","cwd":"/project","agent":"fake"}})).await.json().await.unwrap();
    assert_eq!(body["result"]["isError"], true);
    assert!(body.get("error").is_none());
    assert_eq!(body["result"]["structuredContent"]["code"], "session_limit");
    assert_eq!(body["result"]["structuredContent"]["live"], 1);
    assert!(body["result"]["content"][0]["text"].as_str().unwrap().starts_with("session_limit: 1 of 1 live sessions"));
}
```

- [ ] **Step 3: Run both to verify they fail**

Run: `cargo test -p hennery-sessions external_control::tests::capacity external_control::tests::closed external_control::tests::close_requested external_control::tests::guard_in_uses` and `cargo test -p hennery --test external_mcp limit_refusals`
Expected: compile error on `LimitRefusal`.

- [ ] **Step 4: Implement** `LimitRefusal`, `live_sessions_in` (the existing count query plus `AND s.close_requested = 0`), `capacity_refusal`, and replace both `ensure!(live < 32, "session_limit")` sites with `let live = live_sessions_in(...)?; if live >= i64::from(client.session_capacity) { return Err(capacity_refusal(live, client.session_capacity).into()); }`. Add the `downcast_ref` branch in `external_mcp.rs`. Verify by reading that nothing between `external_admit` and the endpoint re-wraps errors with `anyhow!(e.to_string())` (the path is `external_control::invoke` → `.await??` → endpoint).

- [ ] **Step 5: Run tests**

Run: `cargo test -p hennery-sessions external_control` and `cargo test -p hennery --test external_mcp`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/hennery-sessions/src/external_control_store.rs crates/hennery-sessions/src/external_control.rs crates/hennery-sessions/src/external_mcp.rs crates/hennery/tests/external_mcp.rs
git commit -m "feat(mcp): enforce per-client session capacity with structured refusals"
```

---

### Task 3: Creation rate fuse

**Files:**
- Modify: `crates/hennery-sessions/src/external_control_store.rs` (`external_admit` create/clone branch, after the capacity check and before the `operation_limit` check)
- Modify: `crates/hennery-sessions/src/external_mcp.rs:360` (add `"session_rate_limit" => "session_rate_limit",` to the match)
- Test: `crates/hennery-sessions/src/external_control.rs` tests module

**Interfaces:**
- Consumes: `LimitRefusal`, `bind`, `world_with_capacity` from Task 2.
- Produces in `external_control_store.rs`:
  ```rust
  pub const CREATE_RATE_LIMIT: i64 = 16;
  pub const CREATE_RATE_WINDOW_SECS: i64 = 60;
  /// (creations in window, oldest created_at in window or None)
  pub(crate) fn creations_in_window_in(conn: &rusqlite::Connection, owner: &str, client_id: &str, now: i64) -> anyhow::Result<(i64, Option<i64>)>
  pub(crate) fn rate_refusal(used: i64, retry_after: i64) -> LimitRefusal
  ```
  Query: `SELECT count(*), MIN(created_at) FROM external_operations WHERE owner_id=?1 AND client_id=?2 AND tool IN ('session_create','session_clone') AND created_at > ?3` with `?3 = now - CREATE_RATE_WINDOW_SECS`. `retry_after = max(1, oldest + CREATE_RATE_WINDOW_SECS - now)`. `rate_refusal` builds `code = "session_rate_limit"`, `text = format!("session_rate_limit: {used} sessions created in the last {CREATE_RATE_WINDOW_SECS} s; retry after {retry_after} s")`, `data = json!({"code":"session_rate_limit","limit":CREATE_RATE_LIMIT,"window_secs":CREATE_RATE_WINDOW_SECS,"used":used,"retry_after":retry_after})`.

- [ ] **Step 1: Write the failing tests** in `external_control.rs` tests. Helper `fn create_args(key: &str) -> Value` → `json!({"request_key":key,"host_id":"host","cwd":"/project","agent":"fake"})`.

```rust
#[test]
fn rate_fuse_refuses_seventeenth_creation_in_window_even_with_large_capacity() {
    let (_d, state, identity) = world_with_capacity(1024);
    for n in 0..16 { state.store.external_admit(&identity, "session_create", &format!("k{n}"), &create_args(&format!("k{n}"))).unwrap(); }
    let err = state.store.external_admit(&identity, "session_create", "k16", &create_args("k16")).unwrap_err();
    assert_eq!(err.to_string(), "session_rate_limit");
    let r = err.downcast_ref::<crate::external_control_store::LimitRefusal>().unwrap();
    assert_eq!(r.data["used"], 16); assert_eq!(r.data["limit"], 16); assert_eq!(r.data["window_secs"], 60);
    assert!(r.data["retry_after"].as_i64().unwrap() >= 1 && r.data["retry_after"].as_i64().unwrap() <= 60);
    assert!(r.text.starts_with("session_rate_limit: 16 sessions created in the last 60 s; retry after "));
    state.store.conn().execute("UPDATE external_operations SET created_at = created_at - 61", []).unwrap();
    state.store.external_admit(&identity, "session_create", "k16", &create_args("k16")).unwrap();
}
#[test]
fn same_key_retry_does_not_consume_rate_budget() {
    let (_d, state, identity) = world_with_capacity(1024);
    for n in 0..16 { state.store.external_admit(&identity, "session_create", &format!("k{n}"), &create_args(&format!("k{n}"))).unwrap(); }
    let again = state.store.external_admit(&identity, "session_create", "k0", &create_args("k0")).unwrap();
    assert_eq!(again.request_key_hash, state.store.external_admit(&identity, "session_create", "k0", &create_args("k0")).unwrap().request_key_hash);
    let total: i64 = state.store.conn().query_row("SELECT count(*) FROM external_operations WHERE tool='session_create'", [], |r| r.get(0)).unwrap();
    assert_eq!(total, 16);
}
#[test]
fn clone_counts_toward_rate() {
    let (_d, state, identity) = world_with_capacity(1024);
    let source = bind(&state, &identity, "src", "/project");
    for n in 0..15 { state.store.external_admit(&identity, "session_create", &format!("k{n}"), &create_args(&format!("k{n}"))).unwrap(); }
    // 16 creations so far (1 bound + 15); the clone is the 17th
    let err = state.store.external_admit(&identity, "session_clone", "cl", &json!({"request_key":"cl","source_session_id":source})).unwrap_err();
    assert_eq!(err.to_string(), "session_rate_limit");
}
#[test]
fn prompts_do_not_count_toward_rate() {
    let (_d, state, identity) = world_with_capacity(1024);
    for n in 0..16 { state.store.external_admit(&identity, "session_prompt", &format!("p{n}"), &json!({"request_key":format!("p{n}"),"session_id":"session","text":"hi"})).unwrap(); }
    state.store.external_admit(&identity, "session_create", "k", &create_args("k")).unwrap();
}
#[test]
fn guard_in_does_not_apply_rate() {
    let (_d, state, identity) = world_with_capacity(1024);
    let r = state.store.external_admit(&identity, "session_create", "k", &create_args("k")).unwrap();
    let (command, _) = state.store.external_pending().unwrap().into_iter().find(|(c,_)| c.operation_id == r.operation_id).unwrap();
    state.store.conn().execute("UPDATE external_operations SET created_at = created_at - 61", []).unwrap();
    for n in 0..16 { state.store.external_admit(&identity, "session_create", &format!("k{n}"), &create_args(&format!("k{n}"))).unwrap(); }
    let hat = state.hosts.host("host").unwrap().unwrap().default_hat_id;
    state.store.external_guard_in(&state.store.conn(), &command, Some(("host", "/project", &hat))).unwrap();
}
```

Note: `clone_counts_toward_rate` relies on `bind` admitting through `external_admit`, so its creation row is in the window. If `session_clone` needs the source to have a catalog (`configuration_unavailable`), make the source bound with the fixture used by the existing clone test in this module, or assert the rate error precedes the catalog lookup by placing the rate check before `catalog_in`; the spec puts rate right after capacity, which is before the clone's catalog read only if the builder moves the capacity+rate block above the `if let Some(source)` block. Do that move: both checks need only `client` and `tool`, so compute them first.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p hennery-sessions external_control::tests::rate external_control::tests::same_key external_control::tests::clone_counts external_control::tests::prompts_do_not external_control::tests::guard_in_does_not`
Expected: compile error / wrong error code.

- [ ] **Step 3: Implement** the constants, `creations_in_window_in`, `rate_refusal`, and the check in `external_admit` for `tool ∈ {session_create, session_clone}`: after the capacity check, `let (used, oldest) = creations_in_window_in(&tx, owner, &client.client_id, unix_now())?; if used >= CREATE_RATE_LIMIT { return Err(rate_refusal(used, retry).into()); }`. Add the match arm in `external_mcp.rs`. Do not touch `external_guard_in`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p hennery-sessions external_control` and `cargo test -p hennery-testkit --test external_control`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/hennery-sessions/src/external_control_store.rs crates/hennery-sessions/src/external_control.rs crates/hennery-sessions/src/external_mcp.rs
git commit -m "feat(mcp): add fixed per-client session creation rate fuse"
```

---

### Task 4: Discovery and documentation

**Files:**
- Modify: `crates/hennery-sessions/src/external_mcp.rs:164` (`hennery_capabilities` JSON)
- Modify: `crates/hennery-sessions/src/external_control.rs:233-240` (`session_create` description) and the `session_clone` tuple below it
- Modify: `docs/specs/2026-10-29-external-session-mcp.md:121-128` (consent section) and `:354-358` (limits paragraph)
- Test: `crates/hennery-testkit/tests/external_control.rs` (near the capability assertion at line 305)

**Interfaces:**
- Consumes: `ClientRecord.session_capacity`, `CREATE_RATE_LIMIT`, `CREATE_RATE_WINDOW_SECS`.
- Produces: `hennery_capabilities` result gains `"limits": {"session_capacity": <u32>, "create_rate": {"limit": 16, "window_secs": 60}, "pending_operations": 32, "operation_receipts": 4096, "max_page": 50}`; top-level `max_page` stays.

- [ ] **Step 1: Write the failing test** in testkit `external_control.rs` (harness `Harness::new` issues a client with `NewClient` — add `session_capacity: Some(7)` there or in a dedicated harness constructor):

```rust
#[tokio::test]
async fn capabilities_report_limits_and_tool_descriptions_mention_them() {
    let h = Harness::new(FakeScript::default()).await; // use the same constructor call the neighbouring tests use; issue the harness client with session_capacity: Some(7)
    let caps = h.rpc("hennery_capabilities", json!({})).await;
    assert_eq!(caps["limits"], json!({"session_capacity":7,"create_rate":{"limit":16,"window_secs":60},"pending_operations":32,"operation_receipts":4096,"max_page":50}));
    assert_eq!(caps["max_page"], 50);
    let tools = h.tools().await; // add `async fn tools(&self) -> Vec<Value>` to Harness: POST {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}} with self.token, return result.tools
    for name in ["session_create", "session_clone"] {
        let d = tools.iter().find(|t| t["name"] == name).unwrap()["description"].as_str().unwrap();
        assert!(d.contains("live session capacity") && d.contains("hennery_capabilities.limits"), "{name}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p hennery-testkit --test external_control capabilities_report_limits`
Expected: FAIL on `limits` missing.

- [ ] **Step 3: Implement.** Add `limits` to the capabilities JSON. Append to the `session_create` and `session_clone` descriptions the sentence: ` Counts toward this client's live session capacity (parked sessions included) and a fixed creation rate limit; see hennery_capabilities.limits.`

- [ ] **Step 4: Update the spec.** In `docs/specs/2026-10-29-external-session-mcp.md`, replace the sentence `Clients may have at most 32 live created sessions.` with:

```
Each client has a live-session capacity chosen by the operator at consent
(`session_capacity`, default 32, range 1–1024). Live means created by this
client and not closed, deleted, failed or already asked to close; parked
sessions count. Changing it requires fresh consent (revoke and reconnect).
Independently, a client may be admitted for at most 16 `session_create` or
`session_clone` operations in any rolling 60 s window; this fuse is fixed in
code and cannot be raised by configuration. Refusals are tool results:
`session_limit` (`structuredContent.live`, `.limit`) and `session_rate_limit`
(`.limit`, `.window_secs`, `.used`, `.retry_after`), with the same numbers in
the text. `hennery_capabilities.limits` reports both, the pending-operation
cap (32), the receipt cap (4096) and the page size (50).
```

In the consent section (after the `hennery:sessions:control` paragraph) add one sentence: `Consent with session control also sets the client's live-session capacity (default 32).`

- [ ] **Step 5: Run tests and gates**

Run: `cargo test -p hennery-testkit --test external_control` and `cargo clippy --workspace --all-targets --locked -- -D warnings && cargo fmt --all --check`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/hennery-sessions/src/external_mcp.rs crates/hennery-sessions/src/external_control.rs crates/hennery-testkit/tests/external_control.rs docs/specs/2026-10-29-external-session-mcp.md
git commit -m "docs(mcp): report session capacity and creation rate limits to clients"
```

---

### Task 5: Consent field and client list in the web UI

**Files:**
- Modify: `web/src/screens/ExternalMcp.tsx:14-18` (`Grant` type), state near `:37`, consent POST `:65-77`, control checkbox block `:140-142`, client list line `:154`
- Test: `web/src/screens/ExternalMcp.test.tsx`

**Interfaces:**
- Consumes: REST `session_capacity` on consent POST (number, only when control is checked) and on `GET /api/external-clients` rows (`session_capacity: number`).

- [ ] **Step 1: Write the failing tests** (same helpers `pending`, `fill`, `stubServer` as the file's other tests):

```ts
it('sends session capacity only with session control and defaults it to 32', async () => {
  const p = pending(); p.request.scope += ' hennery:sessions:control'
  const s = stubServer({ [load]: json(200, p), [consent]: json(400, { code: 'expired', message: 'Expired' }) })
  render(<App fetchImpl={s.fetch} />)
  await fill()
  await userEvent.click(screen.getByLabelText('Read session metadata'))
  expect(screen.queryByLabelText('Live sessions this client may hold')).toBeNull()
  await userEvent.click(screen.getByRole('button', { name: 'Allow selected access' }))
  await screen.findByRole('alert')
  expect(s.sent.at(-1)?.body).not.toHaveProperty('session_capacity')
  await userEvent.click(screen.getByLabelText('Create, prompt and control sessions'))
  const capacity = screen.getByLabelText('Live sessions this client may hold')
  expect(capacity).toHaveValue(32)
  await userEvent.clear(capacity); await userEvent.type(capacity, '100')
  await userEvent.click(screen.getByRole('button', { name: 'Allow selected access' }))
  await screen.findByRole('alert')
  expect(s.sent.at(-1)?.body).toMatchObject({ control_sessions: true, session_capacity: 100 })
})
it('management shows each client capacity', async () => {
  history.replaceState(null, '', '/external-mcp')
  const s = stubServer({ 'GET /api/external-clients': json(200, [{ client_id: 'c', label: 'Cap', hat_id: 'hat', roots: [], read_metadata: true, existing_sessions: false, expires_at: 9999999999, revoked: false, control_sessions: true, session_capacity: 40 }]) })
  render(<App fetchImpl={s.fetch} />)
  expect(await screen.findByText(/session control enabled, up to 40 live sessions/)).toBeVisible()
})
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd web && npx vitest run src/screens/ExternalMcp.test.tsx`
Expected: FAIL (label not found).

- [ ] **Step 3: Implement.** `Grant.session_capacity?: number`. New state `const [capacity, setCapacity] = useState(32)`. Under the control checkbox, when `control` is true: a `<label className="field"><span className="field-label">Live sessions this client may hold</span><input className="text-input" type="number" min={1} max={1024} aria-label="Live sessions this client may hold" disabled={busy} value={capacity} onChange={e => setCapacity(Number(e.target.value))} /></label>` and `<p>Parked sessions count. Closed sessions do not.</p>`. Consent POST spreads `...(control && metadata ? { control_sessions: true, session_capacity: capacity } : {})`. Submit button additionally disabled when control is checked and `!(capacity >= 1 && capacity <= 1024)`. Client list line becomes `{g.control_sessions ? `session control enabled, up to ${g.session_capacity ?? 32} live sessions` : 'no session control'}`.

- [ ] **Step 4: Run tests and gates**

Run: `cd web && npx vitest run src/screens/ExternalMcp.test.tsx && npm run typecheck && npx vitest run`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/screens/ExternalMcp.tsx web/src/screens/ExternalMcp.test.tsx
git commit -m "feat(web): set external client session capacity at consent"
```

---

### Task 6: Whole-branch verification

- [ ] **Step 1:** `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
  Expected: all green.
- [ ] **Step 2:** `cd web && npm run typecheck && npx vitest run`
  Expected: all green.
- [ ] **Step 3:** Read the final diff of `external_control_store.rs` once more and confirm `revision` is not written anywhere new and `external_guard_in` has no rate check.
