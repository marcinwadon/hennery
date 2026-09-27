# Teardown and reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** No session can wedge. An adapter that dies, a connection that drops, a host that restarts, or an operator's park/close always takes a session to a truthful, final state: `parked`, `closed` or `failed`, with exactly one end per started turn. The whole adapter subtree dies with it.

**Architecture:** On the host, a new adapter supervisor (`adapter.rs`) owns each adapter process as the leader of its own process group. It captures a scrubbed stderr tail, watches for exit, and kills the whole group. The session actor is rewritten around a single `select!` loop. That loop is the only emitter of the session's frames: adapter notifications reach it through a channel, and the ACP connection runs in a separate task. As a result, a turn runs concurrently with park, close, exit and the idle reaper. On the collector, `resend_complete` triggers `Store::reconcile_host`, which settles starts, turns and closes left in doubt by a drop or a restart. The hub sends nothing to a host until that has run. Operator park and close get REST endpoints.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, tokio-tungstenite 0.29, agent-client-protocol 2.2.0, rusqlite 0.40, libc 0.2 (new in `hennery-host` and `hennery-testkit`: `killpg`), schemars/ts-rs codegen. Nix flake dev shell.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md). The relevant sections are §2.1–2.3 (lifetimes, actor, supervisor), §3.2–3.6 (bodies, catalogue, timeouts, seqs), §4.2, §4.4, §4.7, §4.8 (state machine, turns, reaper, park/close), §5.1–5.4 (handshake, host restart, collector restart) and §12 (scenarios 5, 6, 7, 14, 15, 18). It builds on the executed [walking skeleton](2026-09-26-walking-skeleton.md): read its "Execution status" first, because the code there wins over its task text.

## Execution status (2026-09-27)

**Executed** on branch `feat/teardown-reconciliation` (task-by-task with reviews, then a whole-branch review
and one fix wave; 115 tests). Where review found the plan's code wrong, the **code and the spec win**; the task
bodies below are kept as written. Deviations:

| Area | As built | Why |
|---|---|---|
| Stderr tail (T3) | Once the ring truncates, the partial first line is dropped before scrubbing | A token cut at the ring's edge escaped the scrubber |
| Process group (T3) | The exit watcher SIGKILLs the group right after reaping the leader; `kill_group` is a no-op after exit | Decision 5 inside the supervisor; closes the pgid-recycle window |
| Actor ordering (T4) | Queued updates are drained before every `turn_ended` (reply, failed, teardown, reaper); commands drained with `recv().await` after `close()` | Multi-thread races put an update after its turn end or dropped a command |
| Restart during teardown (T6) | Answered `not_attached` | A start reaching an ending actor was silently dropped (90 s wait, then a reconnect) |
| Late `turn_started` (T7) | Takes the slot back only from a turn still `sent` (that turn becomes `turn_not_delivered`); never reopens an ended turn | Extension of decision 2: otherwise a turn could be orphaned with no end |
| `close_now` (T7) | Resolves an open turn first (synthesized end or not delivered) | A started turn could stay open forever |
| Reconcile close (T9) | A host `not_attached` answer closes the session collector-side | Decision 7 on the reconcile path |
| Unapplied facts (final) | `events.applied` (migration 3); `events()` and SSE replay list only applied rows | Replay could show two ends for one turn |
| Host shutdown (final) | Waits for actors (bounded by kill grace + 1 s), so adapters get SIGTERM first | Shutdown SIGKILLed adapters with no grace |
| Wire order (final) | Outboxed facts are sent before each reply | A rejection could overtake the fact before it |

## Scope

This is **plan A** of the skeleton's "After this plan" item (1). That item was too big for one plan of right-sized tasks, so it is split. Plan A is the reconciliation and teardown core: it removes every known wedge. Plan B (resume) is scoped under "After this plan" below.

**In:**
- Frames: `park_session` / `close_session`; bodies `session_parked`, `session_closed`, `adapter_exited`.
- Host: the adapter supervisor (process group, SIGTERM→SIGKILL, kill on drop, stderr ring buffer, scrubbing) and the exit watcher (ACP core §2.3 steps 1, 2, 4, 5). The session actor ends when its adapter exits.
- Host: host-side `turn_in_progress`; operator park and close; the idle reaper; re-emitting `session_started` for a repeated start; `AttachedSession.open_turn_id`.
- Host connection: pruning ended actors from `hello`; resetting backoff after a healthy period; bounding `connect_async`.
- Collector: §3.6 `conflict` events; the teardown state transitions; a durable close intent; and `resend_complete` reconciliation (`start_not_delivered`, `turn_not_delivered`, `host_restarted` + `turn_ended_synthesized`, re-sent `close_session`).
- Collector: requests gated on reconciliation; a request timeout on a live connection drops that connection; `POST …/park` and `POST …/close`.

**Out** (plan B, or later plans, see "After this plan"): `resume_session` / `session/load` and replay suppression; `committed_seq` in start/resume and the fast-forward before a resumed session's first enqueue; host-offline presumed park; `host_note`; `cancel_turn`; `hello.capabilities` (so the `park` capability gate); anything about pending permissions/elicitations; real auth; hats; gateway; frontend; distribution.

**Skeleton obligations and where they land:**

| Obligation (skeleton "Execution status") | Here |
|---|---|
| Reconcile starts and turns after a drop or timeout (no `starting` / 409 wedge) | Tasks 7, 8 |
| §3.6 `conflict` events | Task 7 |
| Hold `enqueue` for a resumed session until the fast-forward | **Plan B.** Only a resume can meet a lost outbox with an unattached session, and `resume_session` does not exist yet |
| Fill `AttachedSession.open_turn_id` | Tasks 4, 6 |
| Re-emit `session_started` for a retried start | Tasks 4, 6 |
| End the actor when its adapter exits | Task 4 |
| Kill the adapter's process group | Tasks 3, 4 |
| Reset backoff after a healthy period; bound `connect_async` | Task 6 |
| systemd `KillMode=mixed`; TS optionals | Unchanged: distribution and frontend plans |

## Decisions this plan makes where the spec is silent

Confirmed by the maintainer on 2026-09-27. The tasks implement them as written.

1. **A request timeout on a live connection drops that connection** (`Hub::disconnect`). §3.4 says such a timeout is "reported the same way" as a drop, but reconciliation only runs at a handshake. Forcing a reconnect is what makes the timed-out start or turn reconcile at all. The cost is that the other sessions on that host see a reconnect; their adapters are unaffected (§2.1).
2. **A late `turn_started` wins over `not_delivered`.** The turn opens again, because the adapter really has it. This happens if the fact was emitted after the host's resend snapshot.
3. **A started turn still open after a full resend, which the host no longer reports** (reachable with a lost outbox, and also when the `hello` snapshot of attached sessions and the resend are taken at different moments, so a turn whose `turn_started` is emitted in between is resent but not listed as open; the host's reconnect backoff of at least 500 ms makes that window practically unreachable), gets `turn_ended_synthesized{interrupted}`. Leaving it open would wedge the session at 409.
4. **An open turn that never started, on a host that restarted**, becomes `turn_not_delivered`, not `interrupted`: the agent never saw it.
5. **After an unexpected adapter exit, the rest of its process group is SIGKILLed.** Descendants of a crashed adapter are orphans that nothing else reaps (P-5).
6. **Closing a `starting` session** returns 409 `starting` while its host is connected and reconciled. If the host is not connected, the session is closed immediately.
7. **A `session_parked` that overtakes a requested close makes the session `closed`.** A close that the host answers with `not_attached` also closes it collector-side.
8. **Duplicate detection compares bodies structurally** (stored vs received JSON) instead of storing a `payload_hash` column. Key order is not stable across builds, because `serde_json`'s `preserve_order` is feature-unified, and a Rust-std hash is not stable across releases.
9. **Requests to a host that is connected but not yet reconciled** get 409 `host_offline`, the same as a disconnected host. They are refused, not queued. `GET /api/hosts` lists only reconciled hosts.
10. **The reaper's clock** restarts at session start and at each turn end. Adapter notifications outside a turn do not count as activity.
11. **`POST …/park` and `POST …/close`** answer 202 `LifecycleResponse {session_id, lifecycle}` once the fact is ingested. The spec gives no body.
12. **The `park` capability gate is skipped.** `hello` carries no capabilities yet, and every hennery host can park.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- `cargo fmt --all --check` (`max_width = 120`) and `cargo clippy --workspace --all-targets --locked -- -D warnings` pass after every task; `cargo test --workspace --locked` passes after every task.
- Generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated with `cargo run -p hennery-proto --bin gen` whenever a wire type changes and must pass `cargo run -p hennery-proto --bin gen -- --check`. In `codegen.rs`, new root types (frames, REST payloads) go in both `add!` lists and new nested types go in the `render_ts` list.
- `Cargo.lock` is committed whenever a dependency is added (CI runs `--locked`).
- ACP payloads are forwarded verbatim as `serde_json::Value` and never scrubbed; stderr tails are scrubbed (ACP core §2.3).
- Every state-bearing fact from the host is a sequenced `session` frame through the outbox; only rejections (`error`) bypass it (ACP core §3.3).
- The collector acks a frame only after its transaction commits; ingest is idempotent on `(session_id, seq)`.
- Every started turn gets exactly one end: `turn_ended` from the host, or `turn_ended_synthesized` from the collector (host restart, or a lost end) (ACP core §4.4).
- Close, park, reap and host shutdown kill the adapter's **whole process group**: SIGTERM, then SIGKILL after 5 s (`KILL_GRACE`) (ACP core §2.3).
- Adapters strip `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SSE_PORT` from their environment (ACP core §2.3).
- Collector timeouts: start 90 s, prompt 60 s, park/close 60 s, all ≥ the 45 s read deadline; the host bounds a start at 75 s (ACP core §3.4). Idle reap default 30 min, `0` = off (§4.7).
- The collector sends no request to a host before that connection's post-`resend_complete` reconciliation (ACP core §5.1 step 4).
- No global installs: tooling comes from the flake dev shell.
- Commits: Conventional Commits (`feat(host): …`), made with the repository's own identity (gmail, unsigned); push the feature branch after every completed task; never push `main`.

## Review Focus

These are the five inputs most likely to bite a real user that the obvious tests would not exercise. Each is pinned by the named test.

1. **The adapter dies mid-turn**, including the case where its stdout closes before the process exits (the prompt fails first). Expected: exactly one `turn_ended{interrupted}`, never `failed`, then `adapter_exited` with a scrubbed stderr tail, then `session_parked{adapter_exited}`. A later prompt gets 409 `not_attached`, including one sent while the session is still ending; that one is answered at once, not after a 60 s timeout. (Task 4: `an_adapter_crash_mid_turn_interrupts_the_turn_and_parks_the_session`, `a_prompt_that_fails_because_the_adapter_is_dying_ends_interrupted`, `commands_queued_behind_an_ending_actor_are_answered_not_attached`; Task 9: `an_adapter_crash_mid_turn_parks_the_session_with_a_scrubbed_stderr_tail`)
2. **A prompt lost in a connection drop.** After the next handshake the turn is `not_delivered` and the next prompt gets 202, never a permanent 409. (Task 7: `reconcile_releases_a_prompt_that_never_reached_the_adapter`; Task 8: `a_prompt_lost_in_a_drop_is_released_and_the_next_prompt_runs`)
3. **The host restarts mid-turn.** Only after `resend_complete`: `host_restarted`, `turn_ended_synthesized{interrupted}`, `parked`. No adapter is re-spawned. (Task 8: `a_restarted_host_parks_its_sessions_and_interrupts_the_open_turn_only_after_resend`; Task 9: `a_host_restart_mid_turn_parks_and_interrupts_without_respawning`)
4. **Teardown reaches grandchildren.** Park, close, reap and host shutdown kill the agent CLI's own subprocesses, and an adapter that ignores SIGTERM is SIGKILLed after the grace. (Task 3: `terminate_kills_the_whole_process_group`, `terminate_escalates_to_sigkill_when_sigterm_is_ignored`, `dropping_an_adapter_kills_its_group`; Task 4: park/close/drop tests; Task 5: `an_idle_session_is_reaped_its_group_killed_and_parked`)
5. **A request made between a reconnect's `hello` and its `resend_complete`** is refused, never sent. If it were sent, reconciliation would mistake it for one lost on the previous connection. (Task 8: `nothing_is_sent_to_a_host_before_its_reconciliation`)

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/frames.rs`, `rest.rs`, `codegen.rs` | `ParkReason`, teardown bodies, park/close frames, `LifecycleResponse` | 1 |
| `crates/hennery-testkit/src/lib.rs`, `src/bin/hennery-fake-acp.rs` | Fake adapter: crash mid-turn, stderr lines, a grandchild; `pid_alive` | 2 |
| `crates/hennery-host/src/adapter.rs` (new) | `AgentCommand`, spawn in a process group, exit watch, group kill, stderr tail, `scrub` | 3 |
| `crates/hennery-host/src/session.rs` (rewritten) | Session actor: one ordered emitter, concurrent turn, exit watcher, park/close, reaper | 4, 5 |
| `crates/hennery-host/src/connection.rs` | Registry of `SessionHandle`s, `hello` with open turns, park/close/restart dispatch, healthy backoff reset, connect timeout | 4, 5, 6 |
| `crates/hennery/src/main.rs` | `--idle-timeout-secs` | 5 |
| `crates/hennery-sessions/src/store.rs` (rewritten) | Migration 2, conflict events, teardown transitions, close intent, `reconcile_host` | 7 |
| `crates/hennery-sessions/src/hub.rs` (rewritten) | Readiness gating, session-keyed waiters, `disconnect` on timeout | 8, 9 |
| `crates/hennery-sessions/src/ws.rs` | Reconcile on `resend_complete`, resolve park/close waiters, honour `disconnect` | 8 |
| `crates/hennery-sessions/src/api.rs` | `POST …/park`, `POST …/close`, `turn_in_progress` → 409 | 9 |
| `crates/hennery-host/tests/adapter.rs` (new), `crates/hennery-testkit/tests/{host_session,host_connection,reconcile,e2e}.rs`, `crates/hennery-sessions/tests/store.rs`, `crates/hennery-proto/tests/frames.rs` | Tests | all |

All commands run from the repository root inside the dev shell (`nix develop`, or direnv). Work on a feature branch off `main` (e.g. `feat/teardown-reconciliation`). Each task leaves the workspace compiling, clippy-clean and green.

---

### Task 1: Wire types for park, close and adapter exit

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`
- Modify (compile only): `crates/hennery-sessions/src/store.rs`, `crates/hennery-host/src/connection.rs`, `crates/hennery-testkit/tests/host_session.rs`
- Test: `crates/hennery-proto/tests/frames.rs`
- Generated (committed): `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`

**Interfaces:**
- Produces: `hennery_proto::frames::ParkReason { Idle, AdapterExited, Operator }` (snake_case on the wire).
- Produces: `SessionBody::SessionParked { reason: ParkReason }`, `SessionBody::SessionClosed`, `SessionBody::AdapterExited { code: Option<i32>, signal: Option<i32>, stderr_tail: String }`. `kind` tags on the wire: `session_parked`, `session_closed`, `adapter_exited`.
- Produces: `CollectorFrame::ParkSession { request_id, session_id }`, `CollectorFrame::CloseSession { request_id, session_id }` (tags `park_session`, `close_session`).
- Produces: `hennery_proto::rest::LifecycleResponse { session_id: String, lifecycle: String }`.
- Until Task 6 the host refuses park/close with `error{code: "unsupported"}`. Until Task 7 the store records the new bodies without applying them.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, change the import to:

```rust
use hennery_proto::frames::{CollectorFrame, HostFrame, Indexed, ParkReason, SessionBody, TurnOutcome};
```

In `every_collector_frame_round_trips`, add these two entries at the end of the `frames` vector (after the `Ack` entry):

```rust
        CollectorFrame::ParkSession {
            request_id: "r".into(),
            session_id: "s".into(),
        },
        CollectorFrame::CloseSession {
            request_id: "r".into(),
            session_id: "s".into(),
        },
```

Append this test at the end of the file:

```rust
#[test]
fn teardown_bodies_use_the_spec_field_names() {
    let cases = [
        (
            SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
            json!({"kind": "session_parked", "reason": "adapter_exited"}),
        ),
        (SessionBody::SessionClosed, json!({"kind": "session_closed"})),
        (
            SessionBody::AdapterExited {
                code: None,
                signal: Some(9),
                stderr_tail: "boom".into(),
            },
            json!({"kind": "adapter_exited", "signal": 9, "stderr_tail": "boom"}),
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(serde_json::to_value(&body).unwrap(), expected);
        assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), body);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-proto --test frames`
Expected: compile error: `ParkReason` unresolved and no variant `ParkSession` / `SessionParked`.

- [ ] **Step 3: Add the types**

In `crates/hennery-proto/src/frames.rs`, add after `TurnOutcome`:

```rust
/// Why a session was parked (ACP core §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ParkReason {
    Idle,
    AdapterExited,
    Operator,
}
```

Add these variants at the end of `SessionBody`, after `TurnEnded`. The final `}` shown closes the enum:

```rust
    /// The adapter process is gone and the session is detached (idle reap,
    /// adapter exit, or an operator park). Completes `park_session`.
    SessionParked { reason: ParkReason },
    /// The operator closed an attached session. Completes `close_session`.
    SessionClosed,
    /// The adapter exited without being asked to (ACP core §2.3). Followed by
    /// `session_parked{adapter_exited}`. The stderr tail is scrubbed.
    AdapterExited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        stderr_tail: String,
    },
}
```

Add these variants at the end of `CollectorFrame`, after `Ack`. The final `}` closes the enum:

```rust
    /// Completed by `session_parked{operator}` (ACP core §4.8).
    ParkSession {
        request_id: String,
        session_id: String,
    },
    /// Completed by `session_closed` (ACP core §4.8).
    CloseSession {
        request_id: String,
        session_id: String,
    },
}
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// Result of a park or close: the session's lifecycle once the request took
/// effect (`parked` or `closed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LifecycleResponse {
    pub session_id: String,
    pub lifecycle: String,
}
```

In `crates/hennery-proto/src/codegen.rs`, add `rest::LifecycleResponse,` after `rest::ApiError,` in **both** `add!` lists (`render_schema` and `render_ts`). Also add `frames::ParkReason,` after `frames::TurnOutcome,` in the `render_ts` list.

- [ ] **Step 4: Keep the workspace compiling**

The new variants make three matches non-exhaustive. The following arms are correct until later tasks replace them.

`crates/hennery-sessions/src/store.rs`, in `ingest`: replace the arm `SessionBody::AcpUpdate { .. } => {}` with:

```rust
            // Stored on the timeline; their state transitions land with
            // reconciliation.
            SessionBody::AcpUpdate { .. }
            | SessionBody::SessionParked { .. }
            | SessionBody::SessionClosed
            | SessionBody::AdapterExited { .. } => {}
```

Also in `store.rs`, add to `body_kind` after the `TurnEnded` arm:

```rust
        SessionBody::SessionParked { .. } => "session_parked",
        SessionBody::SessionClosed => "session_closed",
        SessionBody::AdapterExited { .. } => "adapter_exited",
```

`crates/hennery-host/src/connection.rs`, in `handle`: add this arm before `CollectorFrame::Ack { session_id, ack_seq } => …`:

```rust
        // Wired to the session actor with teardown; until then a park or
        // close is refused, so the collector's waiter returns at once.
        CollectorFrame::ParkSession { request_id, .. } | CollectorFrame::CloseSession { request_id, .. } => uplink
            .reply(HostFrame::Error {
                request_id,
                code: "unsupported".into(),
                message: "this host cannot park or close sessions yet".into(),
            }),
```

`crates/hennery-testkit/tests/host_session.rs`, in `kinds()`: add after the `SessionBody::TurnEnded { .. } => "turn_ended".to_string(),` arm:

```rust
                SessionBody::SessionParked { reason } => {
                    format!(
                        "session_parked:{}",
                        serde_json::to_value(reason).unwrap().as_str().unwrap()
                    )
                }
                SessionBody::SessionClosed => "session_closed".to_string(),
                SessionBody::AdapterExited { .. } => "adapter_exited".to_string(),
```

- [ ] **Step 5: Regenerate, then run the tests**

Run: `cargo run -p hennery-proto --bin gen && cargo test --workspace && cargo run -p hennery-proto --bin gen -- --check`
Expected: `wrote schema/…`, `wrote web/…`; all tests pass (including `teardown_bodies_use_the_spec_field_names` and the two `generated_*_matches_the_checked_in_copy` tests); `--check` exits 0.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/hennery-proto crates/hennery-sessions/src/store.rs crates/hennery-host/src/connection.rs crates/hennery-testkit/tests/host_session.rs schema web
git commit -m "feat(proto): add park, close and adapter-exit frames"
```

---

### Task 2: Fake adapter: crash mid-turn, stderr lines, a grandchild

**Files:**
- Modify: `crates/hennery-testkit/src/lib.rs`, `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, `crates/hennery-testkit/Cargo.toml`, `crates/hennery-testkit/tests/e2e.rs`
- Test: `crates/hennery-testkit/tests/fake_acp.rs`

**Interfaces:**
- Produces: new `FakeScript` fields, all `#[serde(default)]`: `exit_after_chunks: Option<usize>`, `stderr_lines: Vec<String>` and `grandchild_pid_file: Option<String>`. The `..FakeScript::default()` struct-update syntax must be used wherever a `FakeScript` literal is built.
- Produces: `hennery_testkit::CRASH_EXIT_CODE: i32` (= 3) and `hennery_testkit::pid_alive(pid: i32) -> bool`.
- `exit_after_chunks: Some(n)`: after sending `n` chunks of a prompt, the adapter writes `fake-acp: crashing mid-turn` to stderr and exits with status 3, without answering the prompt.
- `grandchild_pid_file`: at startup the adapter spawns `sleep 600`, detached from its stdio but in its process group, and writes that child's pid to the file.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/fake_acp.rs`:

```rust
#[test]
fn exit_after_chunks_crashes_mid_turn_without_answering_the_prompt() {
    let script = r#"{"chunks":["a","b","c"],"exit_after_chunks":1,"stderr_lines":["using token sk-live-123"]}"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
        .env(hennery_testkit::SCRIPT_ENV, script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in session_requests() {
        writeln!(stdin, "{r}").unwrap();
    }
    // Read until EOF: the process exits, so stdout closes.
    let out: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(hennery_testkit::CRASH_EXIT_CODE));
    let updates = out.iter().filter(|m| m["method"] == "session/update").count();
    assert_eq!(updates, 1, "{out:?}");
    assert!(
        out.iter().all(|m| m["id"] != json!(3)),
        "the prompt must not be answered: {out:?}"
    );
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(
        stderr.contains("sk-live-123") && stderr.contains("crashing"),
        "{stderr}"
    );
}

#[test]
fn grandchild_pid_file_records_a_live_process_in_the_adapters_group() {
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = serde_json::to_string(&hennery_testkit::FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..Default::default()
    })
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
        .env(hennery_testkit::SCRIPT_ENV, script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.parse().ok()) {
            break pid;
        }
        assert!(std::time::Instant::now() < deadline, "no grandchild pid file");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(hennery_testkit::pid_alive(pid));
    // SAFETY: getpgid/killpg on processes this test spawned.
    unsafe {
        assert_eq!(
            libc::getpgid(pid),
            child.id() as i32,
            "grandchild left the adapter's group"
        );
        libc::killpg(child.id() as i32, libc::SIGKILL);
    }
    child.wait().unwrap();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test fake_acp`
Expected: compile error: no field `grandchild_pid_file` on `FakeScript`, no `CRASH_EXIT_CODE`, no `pid_alive`, and unresolved `libc`.

- [ ] **Step 3: Implement**

Add `libc = "0.2"` under `[dependencies]` in `crates/hennery-testkit/Cargo.toml`, after `agent-client-protocol.workspace = true`. Because `[dependencies]` are visible to the crate's tests, no dev-dependency is needed.

Replace `crates/hennery-testkit/src/lib.rs` with:

```rust
//! Test support for hennery: the fake ACP adapter binary and shared helpers.

use serde::{Deserialize, Serialize};

/// Behaviour of `hennery-fake-acp`, passed as JSON in `HENNERY_FAKE_ACP_SCRIPT`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FakeScript {
    /// Text chunks streamed as `agent_message_chunk` updates for every prompt.
    pub chunks: Vec<String>,
    /// Delay before each chunk, in milliseconds.
    #[serde(default)]
    pub chunk_delay_ms: u64,
    /// Crash mid-turn: after sending this many chunks of a prompt, write a
    /// line to stderr and exit with status 3 without answering the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_after_chunks: Option<usize>,
    /// Lines written to stderr at startup (e.g. a fake token, to test that
    /// the host scrubs the stderr tail).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stderr_lines: Vec<String>,
    /// At startup, spawn a long-lived `sleep` child (a grandchild of the
    /// host) in the adapter's process group and write its pid to this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grandchild_pid_file: Option<String>,
}

impl Default for FakeScript {
    fn default() -> Self {
        Self {
            chunks: vec!["Hello".into(), " world".into()],
            chunk_delay_ms: 0,
            exit_after_chunks: None,
            stderr_lines: Vec::new(),
            grandchild_pid_file: None,
        }
    }
}

/// Environment variable carrying the script.
pub const SCRIPT_ENV: &str = "HENNERY_FAKE_ACP_SCRIPT";

/// Exit status of the fake adapter when `exit_after_chunks` fires.
pub const CRASH_EXIT_CODE: i32 = 3;

/// Whether a process with this pid is still alive (signal 0 probe). A zombie
/// counts as alive until its parent reaps it.
pub fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill(2) with signal 0 only checks for existence/permission.
    unsafe { libc::kill(pid, 0) == 0 }
}
```

Replace `crates/hennery-testkit/src/bin/hennery-fake-acp.rs` with:

```rust
//! A scripted ACP agent for tests. Speaks ACP over stdio via the
//! `agent-client-protocol` crate, so the host is tested against the same
//! wire format real adapters use.

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Stdio};
use hennery_testkit::{CRASH_EXIT_CODE, FakeScript, SCRIPT_ENV};
use std::time::Duration;

#[tokio::main]
async fn main() -> agent_client_protocol::Result<()> {
    let script: FakeScript = std::env::var(SCRIPT_ENV)
        .ok()
        .map(|s| serde_json::from_str(&s).expect("valid fake script JSON"))
        .unwrap_or_default();

    for line in &script.stderr_lines {
        eprintln!("{line}");
    }
    if let Some(path) = &script.grandchild_pid_file {
        // Detached from our stdio so it cannot hold the ACP pipes open; it
        // stays in our process group, like an agent CLI's own subprocess.
        // Never waited on by design: it must outlive us unless the host kills
        // the whole group.
        #[allow(clippy::zombie_processes)]
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn grandchild");
        std::fs::write(path, child.id().to_string()).expect("write grandchild pid");
    }

    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx| {
                responder
                    .respond(InitializeResponse::new(req.protocol_version).agent_capabilities(AgentCapabilities::new()))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_req: NewSessionRequest, responder, _cx| {
                responder.respond(NewSessionResponse::new("fake-session-1"))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                async move |req: PromptRequest, responder, cx| {
                    let script = script.clone();
                    let cx2 = cx.clone();
                    cx.spawn(async move {
                        for (sent, chunk) in script.chunks.into_iter().enumerate() {
                            if script.exit_after_chunks == Some(sent) {
                                crash().await;
                            }
                            tokio::time::sleep(Duration::from_millis(script.chunk_delay_ms)).await;
                            cx2.send_notification(SessionNotification::new(
                                req.session_id.clone(),
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                    TextContent::new(chunk),
                                ))),
                            ))?;
                        }
                        if script.exit_after_chunks.is_some() {
                            crash().await;
                        }
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}

/// Exit mid-turn without answering the prompt. The short pause lets the
/// chunks already sent reach stdout first.
async fn crash() -> ! {
    tokio::time::sleep(Duration::from_millis(100)).await;
    eprintln!("fake-acp: crashing mid-turn");
    std::process::exit(CRASH_EXIT_CODE);
}
```

In `crates/hennery-testkit/tests/e2e.rs`, the two `FakeScript { … }` literals (`slow` in `empty_prompts_and_overlapping_prompts_are_refused` and `script` in `a_collector_restart_mid_turn_loses_nothing_and_duplicates_nothing`) each get a last line `..FakeScript::default()` after `chunk_delay_ms: …,`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test fake_acp`
Expected: 4 passed.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-testkit Cargo.lock
git commit -m "test(testkit): let the fake adapter crash mid-turn, write stderr and spawn a grandchild"
```

---

### Task 3: Adapter supervisor

**Files:**
- Create: `crates/hennery-host/src/adapter.rs`
- Modify: `crates/hennery-host/src/lib.rs`, `crates/hennery-host/src/session.rs`, `crates/hennery-host/Cargo.toml`
- Test: `crates/hennery-host/tests/adapter.rs`

**Interfaces:**
- Produces (in `hennery_host::adapter`): `AgentCommand` (moved here unchanged; still re-exported as `hennery_host::AgentCommand` and `hennery_host::session::AgentCommand`), `NESTING_VARS`, `STDERR_TAIL_BYTES` (64 KiB) and `KILL_GRACE` (5 s).
- Produces: `ExitInfo { code: Option<i32>, signal: Option<i32> }` and `AdapterIo { stdin: ChildStdin, stdout: ChildStdout }`.
- Produces `Adapter` with:
  - `Adapter::spawn(&AgentCommand, &Path) -> io::Result<(Adapter, AdapterIo)>`
  - `pgid() -> i32`
  - `async exited(&mut self) -> ExitInfo` (cancel-safe)
  - `async exited_within(&mut self, Duration) -> Option<ExitInfo>`
  - `async terminate(&mut self, grace: Duration) -> ExitInfo` (SIGTERM group → wait → SIGKILL group)
  - `kill_group(&mut self)` (SIGKILL whatever is left; idempotent)
  - `async stderr_tail(&mut self) -> String` (scrubbed, last 64 KiB)
  - `Drop` SIGKILLs the group.
- Produces: `pub fn scrub(&str) -> String`.
- The exit watcher task owns the `Child` and is the only code that reaps it. `kill_on_drop` stays as a backstop for the direct child.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-host/tests/adapter.rs`:

```rust
//! The adapter supervisor against plain shell processes: group kill reaches
//! grandchildren, SIGTERM escalates to SIGKILL, dropping an adapter kills its
//! group, stderr is bounded and scrubbed, nesting variables are stripped.

use hennery_host::adapter::{Adapter, AgentCommand, STDERR_TAIL_BYTES, scrub};
use std::path::Path;
use std::time::{Duration, Instant};

fn sh(script: &str) -> AgentCommand {
    AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes for existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Poll until `pid` is gone (a killed orphan is briefly a zombie until init
/// reaps it).
async fn wait_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "pid {pid} is still alive");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn read_pid(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < deadline, "no pid in {}", path.display());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `sh` that starts a background `sleep` (the grandchild), records its pid,
/// then waits on it.
fn with_grandchild(pid_file: &Path) -> AgentCommand {
    sh(&format!("sleep 600 & echo $! > {}; wait", pid_file.display()))
}

#[tokio::test]
async fn terminate_kills_the_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("gc.pid");
    let (mut adapter, _io) = Adapter::spawn(&with_grandchild(&pid_file), dir.path()).unwrap();
    let grandchild = read_pid(&pid_file).await;
    assert!(alive(grandchild));
    adapter.terminate(Duration::from_secs(2)).await;
    wait_dead(grandchild).await;
}

#[tokio::test]
async fn terminate_escalates_to_sigkill_when_sigterm_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let (mut adapter, _io) = Adapter::spawn(&sh("trap '' TERM; while :; do sleep 0.05; done"), dir.path()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await; // let the trap install
    let began = Instant::now();
    let info = adapter.terminate(Duration::from_millis(300)).await;
    assert!(began.elapsed() >= Duration::from_millis(300), "{:?}", began.elapsed());
    assert_eq!(info.signal, Some(libc::SIGKILL), "{info:?}");
}

#[tokio::test]
async fn dropping_an_adapter_kills_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("gc.pid");
    let (adapter, _io) = Adapter::spawn(&with_grandchild(&pid_file), dir.path()).unwrap();
    let grandchild = read_pid(&pid_file).await;
    drop(adapter);
    wait_dead(grandchild).await;
}

#[tokio::test]
async fn an_unexpected_exit_reports_code_and_a_bounded_scrubbed_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let script = "echo 'early Bearer abc.def' >&2; \
                  head -c 100000 /dev/zero | tr '\\0' x >&2; \
                  echo ' late ghp_abcdefghijklmnop' >&2; exit 7";
    let (mut adapter, _io) = Adapter::spawn(&sh(script), dir.path()).unwrap();
    let info = adapter.exited().await;
    assert_eq!((info.code, info.signal), (Some(7), None));
    let tail = adapter.stderr_tail().await;
    assert!(tail.len() <= STDERR_TAIL_BYTES, "{}", tail.len());
    assert!(
        tail.ends_with(" late ghp_[redacted]\n"),
        "{:?}",
        &tail[tail.len() - 40..]
    );
    assert!(!tail.contains("early"), "the tail keeps only the last bytes");
}

#[tokio::test]
async fn nesting_variables_are_removed_from_the_adapter_environment() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("env.txt");
    let mut cmd = sh(&format!("env > {}", out.display()));
    cmd.env.push(("CLAUDECODE".into(), "1".into()));
    cmd.env.push(("HENNERY_KEEP".into(), "yes".into()));
    let (mut adapter, _io) = Adapter::spawn(&cmd, dir.path()).unwrap();
    adapter.exited().await;
    let env = std::fs::read_to_string(&out).unwrap();
    assert!(env.contains("HENNERY_KEEP=yes"), "{env}");
    assert!(!env.contains("CLAUDECODE="), "{env}");
}

#[test]
fn scrub_redacts_token_like_strings_and_leaves_words_alone() {
    let cases = [
        ("Authorization: Bearer abc.DEF-123", "Authorization: Bearer [redacted]"),
        ("key=sk-ant-api03-abcdefgh end", "key=sk-[redacted] end"),
        (
            "tokens ghp_1234567890abcdef and github_pat_11ABCDEFG_xyz",
            "tokens ghp_[redacted] and github_pat_[redacted]",
        ),
        ("slack xoxb-1234-5678-abcdefgh", "slack xoxb-[redacted]"),
        (
            "use sk-learn and a task-sk-12345678",
            "use sk-learn and a task-sk-12345678",
        ),
        ("zażółć gęślą jaźń", "zażółć gęślą jaźń"),
    ];
    for (input, expected) in cases {
        assert_eq!(scrub(input), expected, "input {input:?}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-host --test adapter`
Expected: compile error: unresolved module `hennery_host::adapter`, unresolved crate `libc`.

- [ ] **Step 3: Implement the supervisor**

Add `libc = "0.2"` to `[dependencies]` in `crates/hennery-host/Cargo.toml`, after `hennery-proto.workspace = true`.

Create `crates/hennery-host/src/adapter.rs`:

```rust
//! Adapter process supervision (ACP core §2.3): spawn in its own process
//! group with a scrubbed environment, capture a bounded stderr tail, watch
//! for exit, and kill the whole group — never just the direct child.

use std::collections::VecDeque;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::watch;

/// Environment variables that make an agent refuse to start or double-report
/// when hennery itself runs inside an agent session (ACP core §2.3).
pub const NESTING_VARS: &[&str] = &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SSE_PORT"];

/// Bytes of adapter stderr kept for `adapter_exited` (ACP core §11).
pub const STDERR_TAIL_BYTES: usize = 64 * 1024;

/// How long a group gets between SIGTERM and SIGKILL (ACP core §2.3).
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// How long to wait for the stderr pipe to drain after the adapter exits.
const STDERR_SETTLE: Duration = Duration::from_millis(500);

/// How to launch an agent's ACP adapter.
#[derive(Debug, Clone)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl AgentCommand {
    /// Parse `"program arg1 arg2"` (whitespace-separated, no quoting).
    pub fn parse(command: &str) -> Option<Self> {
        let mut parts = command.split_whitespace().map(str::to_string);
        let program = parts.next()?;
        Some(Self {
            program,
            args: parts.collect(),
            env: Vec::new(),
        })
    }
}

/// How an adapter process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// The adapter's stdio for the ACP transport.
pub struct AdapterIo {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

/// A running (or exited) adapter process and its process group.
pub struct Adapter {
    pgid: i32,
    exit: watch::Receiver<Option<ExitInfo>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    stderr_done: watch::Receiver<bool>,
    /// Set once the group has been sent SIGKILL; `Drop` then does nothing.
    group_killed: bool,
}

impl Adapter {
    /// Spawn `agent` in `cwd` as the leader of a new process group.
    pub fn spawn(agent: &AgentCommand, cwd: &Path) -> std::io::Result<(Self, AdapterIo)> {
        let mut command = tokio::process::Command::new(&agent.program);
        command
            .args(&agent.args)
            .envs(agent.env.iter().cloned())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        for var in NESTING_VARS {
            command.env_remove(var);
        }
        let mut child = command.spawn()?;
        let pgid = child.id().expect("a just-spawned child has a pid") as i32;
        let io = AdapterIo {
            stdin: child.stdin.take().expect("piped stdin"),
            stdout: child.stdout.take().expect("piped stdout"),
        };

        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let (stderr_tx, stderr_done) = watch::channel(false);
        let mut pipe = child.stderr.take().expect("piped stderr");
        let sink = stderr.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut tail = sink.lock().expect("stderr lock");
                tail.extend(&buf[..n]);
                let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                tail.drain(..excess);
            }
            let _ = stderr_tx.send(true);
        });

        // The exit watcher owns the child: it is the only code that reaps it.
        let (exit_tx, exit) = watch::channel(None);
        tokio::spawn(async move {
            let info = match child.wait().await {
                Ok(status) => ExitInfo {
                    code: status.code(),
                    signal: status.signal(),
                },
                Err(_) => ExitInfo {
                    code: None,
                    signal: None,
                },
            };
            let _ = exit_tx.send(Some(info));
        });

        Ok((
            Self {
                pgid,
                exit,
                stderr,
                stderr_done,
                group_killed: false,
            },
            io,
        ))
    }

    /// The adapter's process group id (its leader's pid).
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Resolves when the adapter process has exited (cancel-safe).
    pub async fn exited(&mut self) -> ExitInfo {
        match self.exit.wait_for(Option::is_some).await {
            Ok(info) => info.expect("waited for Some"),
            // The watcher task is gone (runtime shutting down): treat as exited.
            Err(_) => ExitInfo {
                code: None,
                signal: None,
            },
        }
    }

    /// `Some` if the adapter exits within `limit`.
    pub async fn exited_within(&mut self, limit: Duration) -> Option<ExitInfo> {
        tokio::time::timeout(limit, self.exited()).await.ok()
    }

    /// SIGTERM the whole group, wait up to `grace` for the leader to exit,
    /// then SIGKILL the group (which also reaches descendants that ignored
    /// SIGTERM or outlived the leader).
    pub async fn terminate(&mut self, grace: Duration) -> ExitInfo {
        signal_group(self.pgid, libc::SIGTERM);
        let info = match self.exited_within(grace).await {
            Some(info) => info,
            None => {
                signal_group(self.pgid, libc::SIGKILL);
                self.exited().await
            }
        };
        self.kill_group();
        info
    }

    /// SIGKILL whatever is left of the group. Used after an unexpected exit
    /// too: descendants of a crashed adapter are orphans nobody else reaps.
    pub fn kill_group(&mut self) {
        if !self.group_killed {
            signal_group(self.pgid, libc::SIGKILL);
            self.group_killed = true;
        }
    }

    /// The last `STDERR_TAIL_BYTES` of stderr, scrubbed of token-like
    /// strings. Waits briefly for the pipe to drain if the adapter exited.
    pub async fn stderr_tail(&mut self) -> String {
        let _ = tokio::time::timeout(STDERR_SETTLE, self.stderr_done.wait_for(|done| *done)).await;
        let bytes: Vec<u8> = self.stderr.lock().expect("stderr lock").iter().copied().collect();
        scrub(&String::from_utf8_lossy(&bytes))
    }
}

impl Drop for Adapter {
    /// A dropped actor (host shutdown, panic) must not leave the agent CLI's
    /// subtree running: `kill_on_drop` alone reaches only the direct child.
    fn drop(&mut self) {
        self.kill_group();
    }
}

fn signal_group(pgid: i32, signal: i32) {
    // SAFETY: killpg(2) on a process group this host created. ESRCH (group
    // already gone) is expected and harmless.
    unsafe {
        libc::killpg(pgid, signal);
    }
}

const TOKEN_PREFIXES: &[&str] = &[
    "github_pat_",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "sk-",
    "xoxa-",
    "xoxb-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
];
const REDACTED: &str = "[redacted]";

/// Replace token-like strings (`Bearer …`, `sk-…`, `ghp_…`, `github_pat_…`,
/// `xox?-…`) with `[redacted]`, keeping the prefix so the kind of secret is
/// still visible. Applied to stderr tails and host notes, never to ACP
/// payloads (ACP core §2.3).
pub fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_word = false;
    while i < text.len() {
        let rest = &text[i..];
        if !in_word && let Some((keep, total)) = match_secret(rest) {
            out.push_str(&rest[..keep]);
            out.push_str(REDACTED);
            i += total;
            in_word = true;
            continue;
        }
        let c = rest.chars().next().expect("non-empty rest");
        out.push(c);
        in_word = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        i += c.len_utf8();
    }
    out
}

/// `(bytes to keep, bytes consumed)` if `s` starts with a secret.
fn match_secret(s: &str) -> Option<(usize, usize)> {
    for bearer in ["Bearer ", "bearer "] {
        if let Some(rest) = s.strip_prefix(bearer) {
            let n = token_len(rest);
            if n > 0 {
                return Some((bearer.len(), bearer.len() + n));
            }
        }
    }
    for prefix in TOKEN_PREFIXES {
        if let Some(rest) = s.strip_prefix(prefix) {
            let n = token_len(rest);
            // Short runs are words (`sk-learn`), not keys.
            if n >= 8 {
                return Some((prefix.len(), prefix.len() + n));
            }
        }
    }
    None
}

fn token_len(s: &str) -> usize {
    s.bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'=' | b'+' | b'/'))
        .count()
}
```

Replace `crates/hennery-host/src/lib.rs` with:

```rust
//! The hennery host: runs ACP adapters for one machine and relays their
//! sessions to the collector (ACP core spec §2).

pub mod adapter;
pub mod connection;
pub mod outbox;
pub mod session;
pub mod uplink;

pub use adapter::AgentCommand;
pub use connection::{HostConfig, run};
```

In `crates/hennery-host/src/session.rs`, delete the `NESTING_VARS` constant and the `AgentCommand` struct with its `impl` block, since they now live in `adapter.rs`. Then add this line directly above `use crate::uplink::Uplink;`:

```rust
pub use crate::adapter::{AgentCommand, NESTING_VARS};
```

(`session.rs` keeps spawning its own child until Task 4; only the definitions move.)

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-host --test adapter`
Expected: 6 passed.

- [ ] **Step 5: Revert-probe the group kill**

Temporarily (1) make the body of `Drop for Adapter` empty and (2) in `terminate`, replace `signal_group(self.pgid, libc::SIGTERM);` with `unsafe { libc::kill(self.pgid, libc::SIGTERM); } self.group_killed = true;`. This signals only the leader and suppresses the final group SIGKILL. Run `cargo test -p hennery-host --test adapter`. Expected: `terminate_kills_the_whole_process_group` and `dropping_an_adapter_kills_its_group` FAIL (the `sleep` grandchild survives). Restore the code and re-run: 6 passed.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host Cargo.lock
git commit -m "feat(host): add adapter supervisor with process-group kill and scrubbed stderr tail"
```

---

### Task 4: Session actor: one emitter, exit watcher, park and close

**Files:**
- Modify (rewrite): `crates/hennery-host/src/session.rs`
- Modify: `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Consumes (Task 3): `Adapter`, `ExitInfo` and `KILL_GRACE` from `hennery_host::adapter`.
- Produces `SessionCmd`:
  - `Prompt { request_id, turn_id, content }`
  - `Restart { request_id }`: a repeated `start_session`; re-emits `session_started` with the new request id.
  - `Park { request_id }` and `Close { request_id }`.
- Produces: `SessionOptions { start_timeout: Duration, kill_grace: Duration }`, with `Default` = `START_TIMEOUT` and `KILL_GRACE`. Task 5 adds `idle_timeout`.
- Produces `SessionHandle` (Clone):
  - `send(SessionCmd) -> bool` (`false` once the actor has ended)
  - `is_ended() -> bool`
  - `open_turn_id() -> Option<String>`: set *before* `turn_started` is emitted and cleared *after* `turn_ended`, so `None` means every emitted turn also has its end in the outbox.
- Produces: `session::start(uplink, request_id, session_id, agent, cwd) -> SessionHandle` (default options) and `session::spawn(…, options: SessionOptions) -> SessionHandle`. `start_with_timeout` is removed.
- Behaviour:
  - A prompt during a turn is refused with `error{code: "turn_in_progress"}`.
  - On adapter exit: forward the output still in the pipe, `turn_ended{interrupted}` if a turn was open, `adapter_exited`, `session_parked{adapter_exited}`, and the actor ends.
  - `Park`: `turn_ended{interrupted}` if a turn is open, kill the group, `session_parked{operator}`, end.
  - `Close`: the same, ending with `session_closed`.
  - Dropping every handle (host shutdown) kills the group and emits nothing.
  - Commands still queued when the actor ends are answered `error{code: "not_attached"}`: prompts, parks and closes; a `Restart` is only logged. This covers commands sent during the up-to-5 s kill or the post-exit drain. They are never dropped, because a dropped command costs the collector a 60 s timeout and then a forced reconnect. All exits go through one choke point: `run` calls `drive`, then `commands.close()`, then drains with `try_recv`.
- Why the ACP connection runs in its own task: in `agent-client-protocol` 2.2.0, `connect_with` drops its foreground closure when the background side errors (e.g. on stdout EOF). Teardown must never be skipped, so the closure only hands out the `ConnectionTo<Agent>` and parks; the actor owns the adapter and the loop.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/host_session.rs`, update the existing tests to the new handle API:
- Imports become:
  ```rust
  use hennery_host::session::{self, AgentCommand, SessionCmd, SessionHandle, SessionOptions};
  use hennery_testkit::{FakeScript, SCRIPT_ENV, pid_alive};
  use std::path::Path;
  ```
  (keep the other existing `use` lines).
- Every `tx.send(SessionCmd::Prompt { … }).unwrap();` becomes `assert!(tx.send(SessionCmd::Prompt { … }));`, and `tx.send(prompt()).unwrap();` becomes `assert!(tx.send(prompt()));`.
- In `an_adapter_that_hangs_on_start_reports_start_failed`, replace the `session::start_with_timeout(…, Duration::from_millis(200))` call with:
  ```rust
  let _tx = session::spawn(
      uplink.clone(),
      "r0".into(),
      "s1".into(),
      hanger,
      std::env::temp_dir(),
      SessionOptions {
          start_timeout: Duration::from_millis(200),
          ..SessionOptions::default()
      },
  );
  ```

Then append:

```rust
/// The fake adapter with a script.
fn fake_with(script: &FakeScript) -> AgentCommand {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    fake
}

fn slow_script() -> FakeScript {
    FakeScript {
        chunks: (1..=20).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 100,
        ..FakeScript::default()
    }
}

fn prompt(request_id: &str, turn_id: &str) -> SessionCmd {
    SessionCmd::Prompt {
        request_id: request_id.into(),
        turn_id: turn_id.into(),
        content: vec![json!({"type":"text","text":"hi"})],
    }
}

fn has(kind: &str) -> impl Fn(&[HostFrame]) -> bool + '_ {
    move |frames| kinds(frames).iter().any(|k| k == kind)
}

async fn read_pid(path: &Path) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|s| s.parse().ok()) {
            return pid;
        }
        assert!(tokio::time::Instant::now() < deadline, "no pid file");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_dead(pid: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while pid_alive(pid) {
        assert!(tokio::time::Instant::now() < deadline, "grandchild {pid} survived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_ended(handle: &SessionHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !handle.is_ended() {
        assert!(tokio::time::Instant::now() < deadline, "actor did not end");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn session_with_grandchild(uplink: &Uplink, dir: &Path) -> (SessionHandle, std::path::PathBuf) {
    let pid_file = dir.join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..slow_script()
    };
    let handle = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
        SessionOptions {
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    (handle, pid_file)
}

#[tokio::test]
async fn an_adapter_crash_mid_turn_interrupts_the_turn_and_parks_the_session() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec!["1".into(), "2".into(), "3".into()],
        chunk_delay_ms: 50,
        exit_after_chunks: Some(1),
        stderr_lines: vec!["auth header: Bearer secret-token-123".into()],
        ..FakeScript::default()
    };
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&script),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("session_parked:adapter_exited")).await;
    assert_eq!(
        kinds(&frames),
        [
            "session_started",
            "turn_started",
            "update:1",
            "turn_ended",
            "adapter_exited",
            "session_parked:adapter_exited"
        ]
    );
    for frame in &frames {
        match frame {
            HostFrame::Session {
                body: SessionBody::TurnEnded { outcome, .. },
                ..
            } => assert_eq!(*outcome, TurnOutcome::Interrupted),
            HostFrame::Session {
                body: SessionBody::AdapterExited { code, stderr_tail, .. },
                ..
            } => {
                assert_eq!(*code, Some(hennery_testkit::CRASH_EXIT_CODE));
                assert!(stderr_tail.contains("Bearer [redacted]"), "{stderr_tail}");
                assert!(!stderr_tail.contains("secret-token-123"), "{stderr_tail}");
                assert!(stderr_tail.contains("crashing mid-turn"), "{stderr_tail}");
            }
            _ => {}
        }
    }
    wait_ended(&handle).await;
    assert!(!handle.send(prompt("r2", "t2")), "an ended actor accepts no prompt");
}

#[tokio::test]
async fn park_mid_turn_interrupts_the_turn_and_kills_the_adapters_group() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let (handle, pid_file) = session_with_grandchild(&uplink, dir.path());
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("update:1")).await;
    assert!(handle.send(SessionCmd::Park {
        request_id: "rp".into()
    }));
    let frames = wait_until(&uplink, has("session_parked:operator")).await;
    let kinds = kinds(&frames);
    let tail: Vec<&str> = kinds.iter().rev().take(2).rev().map(String::as_str).collect();
    assert_eq!(tail, ["turn_ended", "session_parked:operator"], "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "turn_ended").count(), 1);
    assert!(
        !kinds.contains(&"adapter_exited".to_string()),
        "a requested kill is not an exit: {kinds:?}"
    );
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

#[tokio::test]
async fn close_emits_session_closed_and_kills_the_adapters_group() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let (handle, pid_file) = session_with_grandchild(&uplink, dir.path());
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(SessionCmd::Close {
        request_id: "rc".into()
    }));
    let frames = wait_until(&uplink, has("session_closed")).await;
    assert_eq!(kinds(&frames), ["session_started", "session_closed"]);
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

#[tokio::test]
async fn dropping_every_handle_kills_the_adapter_without_emitting_anything() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let (handle, pid_file) = session_with_grandchild(&uplink, dir.path());
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    drop(handle); // host shutdown: the registry is gone
    wait_dead(grandchild).await;
    assert_eq!(kinds(&uplink.pending().unwrap()), ["session_started"]);
}

#[tokio::test]
async fn a_prompt_during_a_turn_is_refused_and_the_open_turn_is_reported() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&slow_script()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert_eq!(handle.open_turn_id(), None);
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert_eq!(handle.open_turn_id().as_deref(), Some("t1"));
    assert!(handle.send(prompt("r2", "t2")));
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, HostFrame::Error { ref request_id, ref code, .. } if request_id == "r2" && code == "turn_in_progress"),
        "{reply:?}"
    );
    wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(handle.open_turn_id(), None);
    // The refused prompt did not consume t2: it can run now.
    assert!(handle.send(prompt("r3", "t2")));
    let frames = wait_until(&uplink, |f| kinds(f).iter().filter(|k| *k == "turn_ended").count() == 2).await;
    assert_eq!(kinds(&frames).iter().filter(|k| *k == "turn_started").count(), 2);
}

#[tokio::test]
async fn a_repeated_start_re_emits_session_started_with_the_new_request_id() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
    );
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r9".into()
    }));
    let frames = wait_until(&uplink, |f| {
        kinds(f).iter().filter(|k| *k == "session_started").count() == 2
    })
    .await;
    let ids: Vec<(String, String)> = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::SessionStarted {
                        request_id,
                        agent_session_id,
                    },
                ..
            } => Some((request_id.clone(), agent_session_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        [
            ("r0".to_string(), "fake-session-1".to_string()),
            ("r9".to_string(), "fake-session-1".to_string())
        ]
    );
}

/// The adapter's stdout reaches EOF 300 ms before its process exits, so the
/// prompt fails before the exit watcher sees the exit. The turn must still
/// end `interrupted` (the adapter is gone), not `failed`.
#[tokio::test]
async fn a_prompt_that_fails_because_the_adapter_is_dying_ends_interrupted() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec!["1".into(), "2".into()],
        exit_after_chunks: Some(1),
        ..FakeScript::default()
    };
    let wrapper = AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!("{} ; exec >&- ; sleep 0.3", env!("CARGO_BIN_EXE_hennery-fake-acp")),
        ],
        env: vec![(SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap())],
    };
    let handle = session::start(uplink.clone(), "r0".into(), "s1".into(), wrapper, std::env::temp_dir());
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("session_parked:adapter_exited")).await;
    let outcomes: Vec<TurnOutcome> = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::TurnEnded { outcome, .. },
                ..
            } => Some(*outcome),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes, [TurnOutcome::Interrupted], "{:?}", kinds(&frames));
}

/// Commands that reach an actor while it is ending (its adapter is being
/// killed, or has exited) must be answered, not dropped: the collector would
/// otherwise wait out its timeout and drop the whole host connection.
#[tokio::test]
async fn commands_queued_behind_an_ending_actor_are_answered_not_attached() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    // Queued back to back: the actor sees the park first and ends.
    assert!(handle.send(SessionCmd::Park {
        request_id: "r1".into()
    }));
    assert!(handle.send(prompt("r2", "t2")));
    assert!(handle.send(SessionCmd::Close {
        request_id: "r3".into()
    }));
    let mut refused = Vec::new();
    for _ in 0..2 {
        let reply = tokio::time::timeout(Duration::from_secs(10), replies.recv())
            .await
            .expect("a reply, not silence")
            .unwrap();
        match reply {
            HostFrame::Error { request_id, code, .. } => refused.push((request_id, code)),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        refused,
        [
            ("r2".to_string(), "not_attached".to_string()),
            ("r3".to_string(), "not_attached".to_string())
        ]
    );
    wait_until(&uplink, has("session_parked:operator")).await;
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session`
Expected: compile error: no `SessionHandle`, `SessionOptions` or `session::spawn`, and `SessionCmd::Park { .. }` / `Close { .. }` / `Restart { .. }` do not exist.

- [ ] **Step 3: Rewrite the actor**

Replace `crates/hennery-host/src/session.rs` with:

```rust
//! One session actor per attached session (ACP core §2.2), each owning one
//! adapter process (umbrella §6.9).
//!
//! The actor is the only emitter of the session's frames: adapter
//! notifications are forwarded to it in arrival order and it stamps them
//! into the outbox, interleaved with its own facts (turn start/end, exit,
//! park, close). The ACP connection runs in a task of its own, so a dead
//! adapter never takes the actor's teardown down with it.

use crate::adapter::{Adapter, ExitInfo, KILL_GRACE};
pub use crate::adapter::{AgentCommand, NESTING_VARS};
use crate::uplink::Uplink;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionId,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, UntypedMessage};
use hennery_proto::frames::{HostFrame, Indexed, ParkReason, SessionBody, TurnOutcome};
use serde_json::Value;
use std::collections::HashSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// How long `start` waits for spawn → `initialize` → `session/new` →
/// `session_started` before giving up. Kept below the collector's 90s start
/// timeout (ACP core §3.4) so the host's `start_failed` always beats it.
pub const START_TIMEOUT: Duration = Duration::from_secs(75);

/// A prompt that fails because the adapter died can resolve before the exit
/// watcher reaps the process; wait this long for the exit before calling
/// the turn `failed` rather than `interrupted`.
const EXIT_SETTLE: Duration = Duration::from_millis(500);

/// After an exit, adapter output still in the pipe is forwarded until the
/// notification stream has been quiet this long.
const DRAIN_QUIET: Duration = Duration::from_millis(100);

/// Messages from the connection task to a session actor.
#[derive(Debug)]
pub enum SessionCmd {
    Prompt {
        request_id: String,
        turn_id: String,
        content: Vec<Value>,
    },
    /// A repeated `start_session` for an attached session: re-emit
    /// `session_started` with the new request id, never a second adapter
    /// (ACP core §2.2).
    Restart { request_id: String },
    /// Operator park: end any turn, kill the group, `session_parked{operator}`.
    Park { request_id: String },
    /// Operator close: end any turn, kill the group, `session_closed`.
    Close { request_id: String },
}

/// Tunables of one session actor.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub start_timeout: Duration,
    /// SIGTERM → SIGKILL grace when the actor kills its adapter.
    pub kill_grace: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            start_timeout: START_TIMEOUT,
            kill_grace: KILL_GRACE,
        }
    }
}

/// The connection task's handle on a session actor.
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::UnboundedSender<SessionCmd>,
    open_turn: Arc<Mutex<Option<String>>>,
}

impl SessionHandle {
    /// Queue a command. `false` if the actor has ended.
    pub fn send(&self, cmd: SessionCmd) -> bool {
        self.commands.send(cmd).is_ok()
    }

    /// The actor has ended (its last fact is already in the outbox).
    pub fn is_ended(&self) -> bool {
        self.commands.is_closed()
    }

    /// The turn in flight, for `hello.attached_sessions` (ACP core §5.1).
    /// Set before `turn_started` is emitted and cleared after `turn_ended`,
    /// so `None` means every emitted turn has also been ended.
    pub fn open_turn_id(&self) -> Option<String> {
        self.open_turn.lock().expect("open turn lock").clone()
    }
}

/// Spawn a session actor with default options.
pub fn start(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
) -> SessionHandle {
    spawn(uplink, request_id, session_id, agent, cwd, SessionOptions::default())
}

/// Spawn a session actor. Returns its handle; the actor ends (and the handle
/// reports `is_ended`) after start failure, adapter exit, park, close, or
/// when every handle has been dropped (host shutdown).
pub fn spawn(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    agent: AgentCommand,
    cwd: PathBuf,
    options: SessionOptions,
) -> SessionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let open_turn = Arc::new(Mutex::new(None));
    let actor = Actor {
        uplink,
        session_id,
        open_turn: open_turn.clone(),
        options,
    };
    tokio::spawn(actor.run(request_id, agent, cwd, rx));
    SessionHandle {
        commands: tx,
        open_turn,
    }
}

type Reply = Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

struct Turn {
    id: String,
    reply: Reply,
}

/// Aborts the ACP connection task when the actor ends.
struct AcpTask(tokio::task::JoinHandle<()>);

impl Drop for AcpTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Actor {
    uplink: Uplink,
    session_id: String,
    open_turn: Arc<Mutex<Option<String>>>,
    options: SessionOptions,
}

impl Actor {
    fn emit(&self, body: SessionBody) {
        if let Err(err) = self.uplink.emit(&self.session_id, body) {
            tracing::error!(session_id = %self.session_id, error = %err, "failed to persist a session frame");
        }
    }

    fn set_open_turn(&self, turn_id: Option<String>) {
        *self.open_turn.lock().expect("open turn lock") = turn_id;
    }

    fn start_failed(&self, request_id: String, message: String) {
        tracing::warn!(session_id = %self.session_id, %message, "session start failed");
        self.emit(SessionBody::StartFailed {
            request_id,
            code: "start_failed".into(),
            message,
        });
    }

    async fn run(
        self,
        request_id: String,
        agent: AgentCommand,
        cwd: PathBuf,
        mut commands: mpsc::UnboundedReceiver<SessionCmd>,
    ) {
        self.drive(request_id, agent, cwd, &mut commands).await;
        // The session's last frame is in the outbox. Commands sent while the
        // actor was ending (killing its adapter can take the whole grace)
        // are answered, never dropped: the collector would otherwise wait
        // out its timeout. `close` first, so nothing slips in after the
        // last `try_recv`.
        commands.close();
        while let Ok(cmd) = commands.try_recv() {
            match cmd {
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id } => {
                    self.reject(request_id, "not_attached", "the session has ended on this host".into());
                }
                SessionCmd::Restart { request_id } => {
                    tracing::info!(session_id = %self.session_id, %request_id, "ignoring a start for an ended session");
                }
            }
        }
    }

    /// The actor's life: start, then serve until it parks, closes or ends.
    /// Every return leaves the session's final frame in the outbox.
    async fn drive(
        &self,
        request_id: String,
        agent: AgentCommand,
        cwd: PathBuf,
        commands: &mut mpsc::UnboundedReceiver<SessionCmd>,
    ) {
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
            Ok(spawned) => spawned,
            Err(err) => return self.start_failed(request_id, format!("spawn {}: {err}", agent.program)),
        };
        let (updates_tx, mut updates) = mpsc::unbounded_channel::<Value>();
        let (conn_tx, conn_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (_stop_tx, stop_rx) = oneshot::channel::<()>();
        let transport = ByteStreams::new(io.stdin.compat_write(), io.stdout.compat());
        let session_id = self.session_id.clone();
        let _acp = AcpTask(tokio::spawn(async move {
            let result = Client
                .builder()
                .name("hennery-host")
                // Raw handler: payloads are forwarded verbatim, including
                // update kinds this build does not know (ACP core §2.4).
                .on_receive_notification(
                    async move |msg: UntypedMessage, _cx| {
                        if msg.method == "session/update" {
                            let _ = updates_tx.send(msg.params);
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
                    let _ = conn_tx.send(conn);
                    // Keep the connection up until the actor ends.
                    let _ = stop_rx.await;
                    Ok(())
                })
                .await;
            if let Err(err) = result {
                tracing::debug!(%session_id, error = %err, "ACP connection ended");
            }
        }));

        let Ok(conn) = conn_rx.await else {
            adapter.terminate(self.options.kill_grace).await;
            return self.start_failed(request_id, "the ACP connection could not be set up".into());
        };
        let negotiated = tokio::select! {
            result = tokio::time::timeout(self.options.start_timeout, negotiate(&conn, cwd)) => match result {
                Ok(Ok(agent_session)) => Ok(agent_session),
                Ok(Err(err)) => Err(err.to_string()),
                Err(_) => Err(format!("adapter did not start within {}s", self.options.start_timeout.as_secs())),
            },
            info = adapter.exited() => {
                let tail = adapter.stderr_tail().await;
                Err(format!("adapter exited during start ({}): {}", describe(info), last_lines(&tail, 5)))
            }
        };
        let agent_session = match negotiated {
            Ok(agent_session) => agent_session,
            Err(message) => {
                adapter.terminate(self.options.kill_grace).await;
                return self.start_failed(request_id, message);
            }
        };
        self.emit(SessionBody::SessionStarted {
            request_id,
            agent_session_id: agent_session.to_string(),
        });

        // Prompts are deduplicated by turn_id: a retried delivery after a
        // lost acknowledgement must never run the same turn twice. Only a
        // turn that actually started is recorded, so a corrected retry of a
        // rejected (invalid) prompt with the same turn_id still runs.
        let mut seen_turns = HashSet::new();
        let mut turn: Option<Turn> = None;
        loop {
            tokio::select! {
                // Biased: adapter output already received is emitted before
                // the prompt reply it preceded on the wire.
                biased;
                Some(payload) = updates.recv() => self.emit(update(payload)),
                info = adapter.exited() => {
                    return self.adapter_exited(info, &mut adapter, &mut updates, turn.take()).await;
                }
                cmd = commands.recv() => match cmd {
                    // Every handle dropped: the host is shutting down.
                    None => {
                        adapter.terminate(self.options.kill_grace).await;
                        return;
                    }
                    Some(SessionCmd::Prompt { request_id, turn_id, content }) => {
                        let blocks = match parse_prompt(content) {
                            Ok(blocks) => blocks,
                            Err(message) => {
                                self.reject(request_id, "invalid", message);
                                continue;
                            }
                        };
                        if seen_turns.contains(&turn_id) {
                            tracing::info!(%turn_id, "ignoring duplicate prompt delivery");
                            continue;
                        }
                        if turn.is_some() {
                            self.reject(request_id, "turn_in_progress", "a turn is already running".into());
                            continue;
                        }
                        seen_turns.insert(turn_id.clone());
                        self.set_open_turn(Some(turn_id.clone()));
                        self.emit(SessionBody::TurnStarted { request_id, turn_id: turn_id.clone() });
                        let reply = conn.send_request(PromptRequest::new(agent_session.clone(), blocks)).block_task();
                        turn = Some(Turn { id: turn_id, reply: Box::pin(reply) });
                    }
                    Some(SessionCmd::Restart { request_id }) => self.emit(SessionBody::SessionStarted {
                        request_id,
                        agent_session_id: agent_session.to_string(),
                    }),
                    Some(SessionCmd::Park { .. }) => {
                        self.teardown(&mut adapter, turn.take()).await;
                        return self.emit(SessionBody::SessionParked { reason: ParkReason::Operator });
                    }
                    Some(SessionCmd::Close { .. }) => {
                        self.teardown(&mut adapter, turn.take()).await;
                        return self.emit(SessionBody::SessionClosed);
                    }
                },
                result = next_reply(&mut turn) => {
                    let ended = turn.take().expect("a reply implies a turn");
                    match result {
                        Ok(response) => self.end_turn(ended.id, TurnOutcome::Completed, stop_reason(&response), None),
                        Err(err) => {
                            if let Some(info) = adapter.exited_within(EXIT_SETTLE).await {
                                return self.adapter_exited(info, &mut adapter, &mut updates, Some(ended)).await;
                            }
                            self.end_turn(ended.id, TurnOutcome::Failed, None, Some(err.to_string()));
                        }
                    }
                }
            }
        }
    }

    fn reject(&self, request_id: String, code: &str, message: String) {
        self.uplink.reply(HostFrame::Error {
            request_id,
            code: code.into(),
            message,
        });
    }

    fn end_turn(&self, turn_id: String, outcome: TurnOutcome, stop_reason: Option<String>, error: Option<String>) {
        self.emit(SessionBody::TurnEnded {
            turn_id,
            outcome,
            stop_reason,
            error,
        });
        self.set_open_turn(None);
    }

    /// Park or close: end the turn as interrupted, then kill the group.
    async fn teardown(&self, adapter: &mut Adapter, turn: Option<Turn>) {
        if let Some(turn) = turn {
            self.end_turn(turn.id, TurnOutcome::Interrupted, None, None);
        }
        adapter.terminate(self.options.kill_grace).await;
    }

    /// The exit watcher's steps (ACP core §2.3): outstanding calls fail (the
    /// reply future is dropped), the turn ends `interrupted`, then
    /// `adapter_exited` and `session_parked{adapter_exited}`.
    async fn adapter_exited(
        &self,
        info: ExitInfo,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Value>,
        turn: Option<Turn>,
    ) {
        tracing::warn!(session_id = %self.session_id, exit = %describe(info), "adapter exited");
        // Output the adapter wrote before dying is still in the pipe.
        while let Ok(Some(payload)) = tokio::time::timeout(DRAIN_QUIET, updates.recv()).await {
            self.emit(update(payload));
        }
        if let Some(turn) = turn {
            self.end_turn(
                turn.id,
                TurnOutcome::Interrupted,
                None,
                Some("the adapter exited".into()),
            );
        }
        let stderr_tail = adapter.stderr_tail().await;
        adapter.kill_group();
        self.emit(SessionBody::AdapterExited {
            code: info.code,
            signal: info.signal,
            stderr_tail,
        });
        self.emit(SessionBody::SessionParked {
            reason: ParkReason::AdapterExited,
        });
    }
}

/// The in-flight prompt's reply, or never if no turn is running.
async fn next_reply(turn: &mut Option<Turn>) -> agent_client_protocol::Result<PromptResponse> {
    match turn {
        Some(turn) => (&mut turn.reply).await,
        None => std::future::pending().await,
    }
}

/// `initialize` then `session/new`.
async fn negotiate(conn: &ConnectionTo<Agent>, cwd: PathBuf) -> agent_client_protocol::Result<SessionId> {
    conn.send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await?;
    let created = conn.send_request(NewSessionRequest::new(cwd)).block_task().await?;
    Ok(created.session_id)
}

fn update(payload: Value) -> SessionBody {
    SessionBody::AcpUpdate {
        indexed: Indexed::default(),
        payload,
    }
}

fn stop_reason(response: &PromptResponse) -> Option<String> {
    serde_json::to_value(response.stop_reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
}

fn describe(info: ExitInfo) -> String {
    match (info.code, info.signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "unknown status".into(),
    }
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().rev().take(n).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

/// Validate raw prompt content into ACP content blocks. `Err` carries the
/// user-facing rejection reason for `error{code: invalid}`.
fn parse_prompt(content: Vec<Value>) -> Result<Vec<ContentBlock>, String> {
    let blocks: Result<Vec<ContentBlock>, _> = content.into_iter().map(serde_json::from_value).collect();
    match blocks {
        Ok(blocks) if !blocks.is_empty() => Ok(blocks),
        Ok(_) => Err("empty prompt".to_string()),
        Err(err) => Err(err.to_string()),
    }
}
```

- [ ] **Step 4: Adapt the connection task to `SessionHandle`**

In `crates/hennery-host/src/connection.rs`:
- the `use crate::session::…` line becomes `use crate::session::{self, AgentCommand, SessionCmd, SessionHandle};`
- `type Sessions` becomes `type Sessions = Arc<Mutex<HashMap<String, SessionHandle>>>;`
- in the `StartSession` arm, `let tx = session::start(` becomes `let handle = session::start(` and `map.insert(session_id, tx);` becomes `map.insert(session_id, handle);`
- the `Prompt` arm's lookup and send become:

```rust
            let handle = sessions.lock().expect("sessions lock").get(&session_id).cloned();
            match handle {
                Some(handle)
                    if handle.send(SessionCmd::Prompt {
                        request_id: request_id.clone(),
                        turn_id,
                        content,
                    }) => {}
```

(the `_ => uplink.reply(HostFrame::Error { … not_attached … })` arm stays). Task 6 replaces this dispatch wholesale.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_session`
Expected: 16 passed.

- [ ] **Step 6: Revert-probe the EOF race**

In `session.rs`, change `adapter.exited_within(EXIT_SETTLE)` to `adapter.exited_within(Duration::ZERO)` and run `cargo test -p hennery-testkit --test host_session dying`. Expected: FAIL with `left: [Failed]`, `right: [Interrupted]`. Restore it and re-run: PASS.

Then, in `run`, add `break;` as the first statement of the `while let Ok(cmd) = commands.try_recv()` loop, and run `cargo test -p hennery-testkit --test host_session queued`. Expected: FAIL ("a reply, not silence"). Restore it and re-run: PASS.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host crates/hennery-testkit/tests/host_session.rs
git commit -m "feat(host): watch adapter exit and support park and close in the session actor"
```

---

### Task 5: Idle reaper

**Files:**
- Modify: `crates/hennery-host/src/session.rs`, `crates/hennery-host/src/connection.rs`, `crates/hennery/src/main.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Consumes (Task 4): `SessionOptions`, `session::spawn`.
- Produces: `session::IDLE_TIMEOUT` (30 min); `SessionOptions::idle_timeout: Option<Duration>` (`None` = off; default `Some(IDLE_TIMEOUT)`); `HostConfig::idle_timeout: Duration` (zero = off; default `IDLE_TIMEOUT`); `HostConfig::session_options() -> SessionOptions`; CLI `--idle-timeout-secs <u64>` (default 1800) on `host run` and `up`.
- Behaviour: with no turn in flight for `idle_timeout` since the session started or its last turn ended, the actor kills the group and emits `session_parked{idle}`. A turn in flight (a pending question lives inside one) disables the timer entirely.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
fn reaping(uplink: &Uplink, script: &FakeScript, idle: Option<Duration>) -> SessionHandle {
    session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(script),
        std::env::temp_dir(),
        SessionOptions {
            idle_timeout: idle,
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    )
}

#[tokio::test]
async fn an_idle_session_is_reaped_its_group_killed_and_parked() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..FakeScript::default()
    };
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = reaping(&uplink, &script, Some(Duration::from_millis(300)));
    let grandchild = read_pid(&pid_file).await;
    let frames = wait_until(&uplink, has("session_parked:idle")).await;
    assert_eq!(kinds(&frames), ["session_started", "session_parked:idle"]);
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

#[tokio::test]
async fn the_reaper_never_parks_a_session_mid_turn() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    // A 2 s turn against a 300 ms idle window.
    let handle = reaping(&uplink, &slow_script(), Some(Duration::from_millis(300)));
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        !has("session_parked:idle")(&uplink.pending().unwrap()),
        "reaped mid-turn"
    );
    // Once the turn is over, the window starts again and the reaper parks it.
    let frames = wait_until(&uplink, has("session_parked:idle")).await;
    let kinds = kinds(&frames);
    let ended = kinds.iter().position(|k| k == "turn_ended").expect("turn ended");
    let parked = kinds.iter().position(|k| k == "session_parked:idle").unwrap();
    assert!(ended < parked, "{kinds:?}");
}

#[tokio::test]
async fn a_disabled_reaper_leaves_an_idle_session_attached() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = reaping(&uplink, &FakeScript::default(), None);
    wait_until(&uplink, has("session_started")).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(kinds(&uplink.pending().unwrap()), ["session_started"]);
    assert!(!handle.is_ended());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session reap`
Expected: compile error: no field `idle_timeout` on `SessionOptions`.

- [ ] **Step 3: Implement the reaper**

In `crates/hennery-host/src/session.rs`:
- add `use tokio::time::Instant;` after `use tokio::sync::{mpsc, oneshot};`
- add after `START_TIMEOUT`:

```rust
/// Default idle window before the reaper parks a session (ACP core §4.7).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
```

- `SessionOptions` and its `Default` become:

```rust
pub struct SessionOptions {
    pub start_timeout: Duration,
    /// SIGTERM → SIGKILL grace when the actor kills its adapter.
    pub kill_grace: Duration,
    /// Park the session after this long with no turn in flight
    /// (`session_parked{idle}`); `None` disables the reaper.
    pub idle_timeout: Option<Duration>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            start_timeout: START_TIMEOUT,
            kill_grace: KILL_GRACE,
            idle_timeout: Some(IDLE_TIMEOUT),
        }
    }
}
```

- directly before `loop {` in `Actor::run`, after `let mut turn: Option<Turn> = None;`, add:

```rust
        // The reaper's clock: restarted when the session starts and when a
        // turn ends; it never runs while a turn (or a pending question
        // inside one) is in flight.
        let mut idle_since = Instant::now();
```

- in the `result = next_reply(&mut turn) =>` arm, directly after `let ended = turn.take().expect("a reply implies a turn");`, add `idle_since = Instant::now();`
- add a last arm to the `select!`, after the `next_reply` arm:

```rust
                _ = idle_deadline(self.options.idle_timeout, idle_since), if turn.is_none() => {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    adapter.terminate(self.options.kill_grace).await;
                    return self.emit(SessionBody::SessionParked { reason: ParkReason::Idle });
                }
```

- add before `next_reply`:

```rust
/// Resolves when the idle window since `since` has passed; never if the
/// reaper is off.
async fn idle_deadline(window: Option<Duration>, since: Instant) {
    match window {
        Some(window) => tokio::time::sleep_until(since + window).await,
        None => std::future::pending().await,
    }
}
```

In `crates/hennery-host/src/connection.rs`:
- the `use crate::session::…` line becomes `use crate::session::{self, AgentCommand, SessionCmd, SessionHandle, SessionOptions};`
- add a field to `HostConfig`, after `read_timeout`:
  ```rust
      /// Idle reaper window (ACP core §4.7); zero turns the reaper off.
      pub idle_timeout: Duration,
  ```
- in `HostConfig::new`, add `idle_timeout: session::IDLE_TIMEOUT,` after `read_timeout: …,`, and add this method to the `impl HostConfig` block:

```rust
    /// Options for every session actor this host spawns.
    pub fn session_options(&self) -> SessionOptions {
        SessionOptions {
            idle_timeout: (!self.idle_timeout.is_zero()).then_some(self.idle_timeout),
            ..SessionOptions::default()
        }
    }
```
- in the `StartSession` arm, `session::start(` becomes `session::spawn(`, with `cfg.session_options(),` added as the last argument after `PathBuf::from(cwd),`.

In `crates/hennery/src/main.rs`:
- add to both `HostArgs` and `UpArgs`, after the `agents` field:
  ```rust
      /// Park sessions idle for this many seconds; 0 turns the reaper off.
      #[arg(long, default_value_t = 1800)]
      idle_timeout_secs: u64,
  ```
- in `run_host`, after `cfg.agents = …;`, add `cfg.idle_timeout = std::time::Duration::from_secs(args.idle_timeout_secs);`
- in `run_up`, after `.arg(args.data_dir.join("host"))` on `host_cmd`, add `.arg("--idle-timeout-secs").arg(args.idle_timeout_secs.to_string())`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_session`
Expected: 19 passed.

- [ ] **Step 5: Revert-probe the mid-turn guard**

Remove `, if turn.is_none()` from the reaper arm and run `cargo test -p hennery-testkit --test host_session mid_turn`. Expected: `the_reaper_never_parks_a_session_mid_turn` FAILS ("reaped mid-turn"). Restore and re-run: PASS.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host crates/hennery/src/main.rs crates/hennery-testkit/tests/host_session.rs
git commit -m "feat(host): park idle sessions with a configurable reaper"
```

---

### Task 6: Host connection: live sessions in `hello`, park/close dispatch, liveness fixes

**Files:**
- Modify: `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Consumes (Tasks 4–5): `SessionHandle::{send, is_ended, open_turn_id}`, `SessionCmd::{Restart, Park, Close}`, `HostConfig::session_options`.
- Produces: `HostConfig::connect_timeout: Duration` (default 10 s; bounds TCP + WebSocket handshake) and `HostConfig::healthy_after: Duration` (default 60 s; a connection up that long resets the backoff even with no ack).
- Behaviour:
  - `hello.attached_sessions` lists only running actors (ended ones are pruned), sorted by id, each with `open_turn_id`.
  - A repeated `start_session` for a running actor sends `Restart` (no second adapter). A start for an ended one spawns a fresh actor.
  - `park_session` / `close_session` go to the running actor, and otherwise are answered `error{code: "not_attached"}` (as are prompts).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
type ServerSink = futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>;

/// Accept one host connection: read its `hello`, answer `hello_ack`, and
/// read up to its `resend_complete`. Returns the hello's attached sessions.
async fn accept_host(
    listener: &TcpListener,
) -> (ServerSink, ServerStream, Vec<hennery_proto::frames::AttachedSession>) {
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
    let (mut sink, mut stream) = ws.split();
    let HostFrame::Hello { attached_sessions, .. } = read_host_frame(&mut stream).await else {
        panic!("expected hello");
    };
    send_frame(&mut sink, &hello_ack()).await;
    read_until(&mut stream, |f| matches!(f, HostFrame::ResendComplete)).await;
    (sink, stream, attached_sessions)
}

async fn send_frame(sink: &mut ServerSink, frame: &CollectorFrame) {
    sink.send(Message::text(serde_json::to_string(frame).unwrap()))
        .await
        .unwrap();
}

/// Read frames until one matches (10 s bound).
async fn read_until(stream: &mut ServerStream, pred: impl Fn(&HostFrame) -> bool) -> HostFrame {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    let frame: HostFrame = serde_json::from_str(&text).unwrap();
                    if pred(&frame) {
                        return frame;
                    }
                }
                Some(Ok(_)) => {}
                other => panic!("connection ended while waiting: {other:?}"),
            }
        }
    })
    .await
    .expect("expected frame within 10s")
}

fn body_is(session: &str, kind: &str) -> impl Fn(&HostFrame) -> bool {
    let (session, kind) = (session.to_string(), kind.to_string());
    move |f| match f {
        HostFrame::Session { session_id, body, .. } => {
            session_id == &session && serde_json::to_value(body).unwrap()["kind"] == kind.as_str()
        }
        _ => false,
    }
}

fn error_for(request: &str) -> impl Fn(&HostFrame) -> bool {
    let request = request.to_string();
    move |f| matches!(f, HostFrame::Error { request_id, .. } if *request_id == request)
}

fn start(request_id: &str, session_id: &str) -> CollectorFrame {
    CollectorFrame::StartSession {
        request_id: request_id.into(),
        session_id: session_id.into(),
        agent: "fake".into(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
    }
}

/// A host with the fake adapter (a slow 2 s turn) registered as `fake`.
fn host_with_fake(addr: std::net::SocketAddr, name: &str, program: hennery_host::AgentCommand) -> HostConfig {
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        "token",
        unique_data_dir(name),
    );
    cfg.reconnect_min = Duration::from_millis(50);
    cfg.reconnect_max = Duration::from_millis(200);
    cfg.agents.insert("fake".into(), program);
    cfg
}

fn slow_fake() -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        chunks: (1..=20).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 100,
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

#[tokio::test]
async fn hello_reports_live_sessions_with_their_open_turn_and_drops_ended_ones() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "hello-open-turn", slow_fake())));

    let (mut sink, mut stream, attached) = accept_host(&listener).await;
    assert!(attached.is_empty());
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::Prompt {
            request_id: "r2".into(),
            session_id: "s1".into(),
            turn_id: "t1".into(),
            content: vec![serde_json::json!({"type": "text", "text": "go"})],
        },
    )
    .await;
    read_until(&mut stream, body_is("s1", "turn_started")).await;
    send_frame(&mut sink, &start("r3", "s2")).await;
    read_until(&mut stream, body_is("s2", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r4".into(),
            session_id: "s2".into(),
        },
    )
    .await;
    read_until(&mut stream, body_is("s2", "session_closed")).await;
    drop((sink, stream)); // the connection drops mid-turn

    let (_sink, _stream, attached) = accept_host(&listener).await;
    assert_eq!(attached.len(), 1, "{attached:?}");
    assert_eq!(attached[0].session_id, "s1");
    assert_eq!(attached[0].open_turn_id.as_deref(), Some("t1"));
    assert!(attached[0].last_seq >= 2, "{attached:?}");
}

#[tokio::test]
async fn park_reaches_the_actor_and_frames_for_a_detached_session_are_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "park", slow_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    let park = |request_id: &str| CollectorFrame::ParkSession {
        request_id: request_id.into(),
        session_id: "s1".into(),
    };
    send_frame(&mut sink, &park("r2")).await;
    let parked = read_until(&mut stream, body_is("s1", "session_parked")).await;
    assert!(
        matches!(&parked, HostFrame::Session { body: hennery_proto::frames::SessionBody::SessionParked { reason }, .. }
            if *reason == hennery_proto::frames::ParkReason::Operator),
        "{parked:?}"
    );
    // The actor is gone: a second park, a close and a prompt are all refused.
    send_frame(&mut sink, &park("r3")).await;
    let refused = read_until(&mut stream, error_for("r3")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r4".into(),
            session_id: "s1".into(),
        },
    )
    .await;
    read_until(&mut stream, error_for("r4")).await;
}

#[tokio::test]
async fn a_repeated_start_session_re_emits_session_started_without_a_second_adapter() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    // Counts adapter launches, then becomes the fake adapter.
    let counting = hennery_host::AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "echo spawned >> {}; exec {}",
                spawns.display(),
                env!("CARGO_BIN_EXE_hennery-fake-acp")
            ),
        ],
        env: Vec::new(),
    };
    tokio::spawn(run(host_with_fake(addr, "restart", counting)));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    send_frame(&mut sink, &start("r2", "s1")).await;
    let mut request_ids = Vec::new();
    for _ in 0..2 {
        if let HostFrame::Session {
            body: hennery_proto::frames::SessionBody::SessionStarted { request_id, .. },
            ..
        } = read_until(&mut stream, body_is("s1", "session_started")).await
        {
            request_ids.push(request_id);
        }
    }
    assert_eq!(request_ids, ["r1", "r2"]);
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 1);
}

/// Acks `hello`, keeps the connection up for `hold`, then closes cleanly,
/// never acking a frame. Reports each hello's arrival time.
async fn hold_then_close_server(listener: TcpListener, hold: Duration, hellos: mpsc::UnboundedSender<Instant>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(Instant::now()).ok();
            let _ = sink
                .send(Message::text(serde_json::to_string(&hello_ack()).unwrap()))
                .await;
            tokio::time::sleep(hold).await;
            let _ = sink.close().await;
        });
    }
}

#[tokio::test]
async fn backoff_resets_after_a_healthy_connection_even_without_acks() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, mut hellos) = mpsc::unbounded_channel();
    // Up for 150 ms each time, against a 100 ms healthy threshold.
    tokio::spawn(hold_then_close_server(listener, Duration::from_millis(150), tx));

    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        "token",
        unique_data_dir("healthy-reset"),
    );
    cfg.reconnect_min = Duration::from_millis(20);
    cfg.reconnect_max = Duration::from_secs(10);
    cfg.healthy_after = Duration::from_millis(100);
    tokio::spawn(run(cfg));

    let mut timestamps = Vec::new();
    for _ in 0..6 {
        let t = tokio::time::timeout(Duration::from_secs(3), hellos.recv())
            .await
            .expect("hello within 3s")
            .expect("hello channel open");
        timestamps.push(t);
    }
    // Each gap is the 150 ms hold plus the backoff; without the reset the
    // backoff alone would pass 300 ms by the fifth reconnect.
    let gaps: Vec<Duration> = timestamps.windows(2).map(|w| w[1] - w[0]).collect();
    for (i, gap) in gaps.iter().enumerate() {
        assert!(*gap < Duration::from_millis(350), "gap {i} was {gap:?}; gaps {gaps:?}");
    }
}

#[tokio::test]
async fn a_collector_that_never_completes_the_handshake_is_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut cfg = HostConfig::new(
        format!("ws://{addr}/api/hosts/ws"),
        "host1",
        "token",
        unique_data_dir("connect-timeout"),
    );
    cfg.connect_timeout = Duration::from_millis(200);
    cfg.reconnect_min = Duration::from_millis(50);
    tokio::spawn(run(cfg));

    // Accept TCP but never answer the WebSocket upgrade.
    let mut held = Vec::new();
    for attempt in 0..2 {
        let (tcp, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap_or_else(|_| panic!("connection attempt {attempt} within 2s"))
            .unwrap();
        held.push(tcp);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test host_connection`
Expected: compile error: no field `healthy_after` / `connect_timeout` on `HostConfig`.

- [ ] **Step 3: Implement**

Replace `crates/hennery-host/src/connection.rs` with:

```rust
//! The host's connection to the collector (ACP core §2, §5).
//!
//! Adapters belong to the host process, not to this connection: a dropped
//! socket only ends `connect_once`; session actors keep running and keep
//! writing to the outbox, which is resent on the next connection.

use crate::outbox::Outbox;
use crate::session::{self, AgentCommand, SessionCmd, SessionHandle, SessionOptions};
use crate::uplink::Uplink;
use anyhow::{Context, Result, bail};
use futures::{SinkExt, StreamExt};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct HostConfig {
    /// e.g. `ws://127.0.0.1:7117/api/hosts/ws`
    pub collector_url: String,
    pub host_id: String,
    /// Walking skeleton: shared development token (ACP core §3.5 replaces it).
    pub token: String,
    pub data_dir: PathBuf,
    pub agents: HashMap<String, AgentCommand>,
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    pub ping_interval: Duration,
    pub read_timeout: Duration,
    /// Idle reaper window (ACP core §4.7); zero turns the reaper off.
    pub idle_timeout: Duration,
    /// Bound on the TCP + WebSocket handshake of one connection attempt.
    pub connect_timeout: Duration,
    /// A connection that stays up this long resets the reconnect backoff,
    /// even if nothing was acked (an idle host sends nothing to ack).
    pub healthy_after: Duration,
}

impl HostConfig {
    pub fn new(
        collector_url: impl Into<String>,
        host_id: impl Into<String>,
        token: impl Into<String>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            collector_url: collector_url.into(),
            host_id: host_id.into(),
            token: token.into(),
            data_dir,
            agents: HashMap::new(),
            reconnect_min: Duration::from_millis(500),
            reconnect_max: Duration::from_secs(30),
            ping_interval: Duration::from_secs(15),
            read_timeout: Duration::from_secs(45),
            idle_timeout: session::IDLE_TIMEOUT,
            connect_timeout: Duration::from_secs(10),
            healthy_after: Duration::from_secs(60),
        }
    }

    /// Options for every session actor this host spawns.
    pub fn session_options(&self) -> SessionOptions {
        SessionOptions {
            idle_timeout: (!self.idle_timeout.is_zero()).then_some(self.idle_timeout),
            ..SessionOptions::default()
        }
    }
}

type Sessions = Arc<Mutex<HashMap<String, SessionHandle>>>;

/// Run the host until the process exits. Reconnects with exponential backoff.
pub async fn run(cfg: HostConfig) -> Result<()> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let outbox = Outbox::open(&cfg.data_dir.join("outbox.db"))?;
    let (uplink, mut replies) = Uplink::new(outbox);
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let mut backoff = cfg.reconnect_min;
    loop {
        if let Err(err) = connect_once(&cfg, &uplink, &sessions, &mut replies, &mut backoff).await {
            tracing::warn!(error = %err, "collector connection ended");
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(cfg.reconnect_max);
    }
}

async fn connect_once(
    cfg: &HostConfig,
    uplink: &Uplink,
    sessions: &Sessions,
    replies: &mut mpsc::UnboundedReceiver<HostFrame>,
    backoff: &mut Duration,
) -> Result<()> {
    let (ws, _) = tokio::time::timeout(
        cfg.connect_timeout,
        tokio_tungstenite::connect_async(&cfg.collector_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("no WebSocket handshake within {:?}", cfg.connect_timeout))?
    .context("connect to collector")?;
    let (mut sink, mut stream) = ws.split();

    let attached = attached_sessions(uplink, sessions)?;
    send(
        &mut sink,
        &HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            host_id: cfg.host_id.clone(),
            token: cfg.token.clone(),
            attached_sessions: attached,
        },
    )
    .await?;

    match tokio::time::timeout(cfg.read_timeout, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<CollectorFrame>(&text)? {
            CollectorFrame::HelloAck { committed, .. } => {
                for (session_id, seq) in committed {
                    uplink.fast_forward(&session_id, seq)?;
                }
            }
            CollectorFrame::HelloError { code, message } => bail!("hello rejected: {code}: {message}"),
            other => bail!("expected hello_ack, got {other:?}"),
        },
        other => bail!("no hello_ack: {other:?}"),
    }
    // A bare handshake is not proof the collector is actually committing
    // anything: a persistently failing collector (e.g. disk full) can still
    // ack `hello` and then drop every subsequent frame without acking it
    // (ws.rs never acks past a failed ingest). Resetting backoff here would
    // make the host hammer such a collector at `reconnect_min` forever;
    // instead it is reset below, only once the first real `ack` lands.
    tracing::info!(collector = %cfg.collector_url, "connected to collector");

    // Resend everything unacked, then tell the collector we are done.
    let mut sent: HashMap<String, u64> = HashMap::new();
    send_pending(&mut sink, uplink, &mut sent).await?;
    send(&mut sink, &HostFrame::ResendComplete).await?;

    let mut ping = tokio::time::interval(cfg.ping_interval);
    ping.tick().await;
    // Tracks the deadline independently of `select!`'s per-iteration futures:
    // rebuilding `timeout(stream.next())` fresh every loop (as the earlier
    // version did) restarts its clock on every unrelated arm (a ping tick, an
    // outbox wakeup, a reply), so the "no frame in read_timeout" branch could
    // never actually fire. `deadline` only moves when a frame arrives.
    let mut deadline = Instant::now() + cfg.read_timeout;
    // An idle host has nothing to ack, so a connection that simply stays up
    // is proof enough too (the ack-based reset below covers busy hosts).
    let healthy = tokio::time::sleep(cfg.healthy_after);
    tokio::pin!(healthy);
    let mut proven = false;
    loop {
        tokio::select! {
            _ = &mut healthy, if !proven => {
                proven = true;
                *backoff = cfg.reconnect_min;
            }
            _ = uplink.changed() => send_pending(&mut sink, uplink, &mut sent).await?,
            Some(frame) = replies.recv() => send(&mut sink, &frame).await?,
            _ = ping.tick() => sink.send(Message::Ping(Default::default())).await?,
            _ = tokio::time::sleep_until(deadline) => bail!("no frame from collector within {:?}", cfg.read_timeout),
            msg = stream.next() => {
                deadline = Instant::now() + cfg.read_timeout;
                match msg {
                    None => bail!("collector closed the connection"),
                    Some(Err(err)) => return Err(err.into()),
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<CollectorFrame>(&text) {
                        Ok(frame) => {
                            if matches!(frame, CollectorFrame::Ack { .. }) {
                                // Proof the collector is actually committing
                                // frames, not just accepting the handshake:
                                // only now is it safe to forget the backoff
                                // accumulated from earlier failed attempts.
                                *backoff = cfg.reconnect_min;
                            }
                            handle(cfg, uplink, sessions, frame)?
                        }
                        Err(err) => tracing::warn!(error = %err, "ignoring unknown or invalid frame"),
                    },
                    Some(Ok(Message::Close(_))) => bail!("collector closed the connection"),
                    Some(Ok(_)) => {} // ping/pong/binary: liveness only
                }
            },
        }
    }
}

/// `hello.attached_sessions`: every session whose actor is still running,
/// with its open turn (ACP core §5.1). Ended actors are pruned here.
fn attached_sessions(uplink: &Uplink, sessions: &Sessions) -> Result<Vec<AttachedSession>> {
    let live: Vec<(String, SessionHandle)> = {
        let mut map = sessions.lock().expect("sessions lock");
        map.retain(|_, handle| !handle.is_ended());
        map.iter().map(|(id, h)| (id.clone(), h.clone())).collect()
    };
    let mut out = Vec::new();
    for (session_id, handle) in live {
        out.push(AttachedSession {
            last_seq: uplink.last_seq(&session_id)?,
            open_turn_id: handle.open_turn_id(),
            session_id,
        });
    }
    out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(out)
}

/// The running actor for `session_id`, if any.
fn live_session(sessions: &Sessions, session_id: &str) -> Option<SessionHandle> {
    sessions
        .lock()
        .expect("sessions lock")
        .get(session_id)
        .filter(|h| !h.is_ended())
        .cloned()
}

fn not_attached(uplink: &Uplink, request_id: String) {
    uplink.reply(HostFrame::Error {
        request_id,
        code: "not_attached".into(),
        message: "session is not attached on this host".into(),
    });
}

fn handle(cfg: &HostConfig, uplink: &Uplink, sessions: &Sessions, frame: CollectorFrame) -> Result<()> {
    match frame {
        CollectorFrame::StartSession {
            request_id,
            session_id,
            agent,
            cwd,
        } => {
            let Some(command) = cfg.agents.get(&agent).cloned() else {
                uplink.reply(HostFrame::Error {
                    request_id,
                    code: "unknown_agent".into(),
                    message: format!("agent {agent} is not configured on this host"),
                });
                return Ok(());
            };
            let mut map = sessions.lock().expect("sessions lock");
            // Idempotent (ACP core §2.2): a repeated start for an attached
            // session re-emits `session_started` with the new request id and
            // never spawns a second adapter.
            if let Some(handle) = map.get(&session_id).filter(|h| !h.is_ended())
                && handle.send(SessionCmd::Restart {
                    request_id: request_id.clone(),
                })
            {
                return Ok(());
            }
            let handle = session::spawn(
                uplink.clone(),
                request_id,
                session_id.clone(),
                command,
                PathBuf::from(cwd),
                cfg.session_options(),
            );
            map.insert(session_id, handle);
        }
        CollectorFrame::Prompt {
            request_id,
            session_id,
            turn_id,
            content,
        } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Prompt {
                    request_id: request_id.clone(),
                    turn_id,
                    content,
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::ParkSession { request_id, session_id } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Park {
                    request_id: request_id.clone(),
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::CloseSession { request_id, session_id } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Close {
                    request_id: request_id.clone(),
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
        CollectorFrame::HelloAck { .. } | CollectorFrame::HelloError { .. } => {}
    }
    Ok(())
}

async fn send_pending<S>(sink: &mut S, uplink: &Uplink, sent: &mut HashMap<String, u64>) -> Result<()>
where
    S: futures::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    for frame in uplink.pending()? {
        if let HostFrame::Session { session_id, seq, .. } = &frame {
            if sent.get(session_id).is_some_and(|last| seq <= last) {
                continue;
            }
            sent.insert(session_id.clone(), *seq);
        }
        send(sink, &frame).await?;
    }
    Ok(())
}

async fn send<S>(sink: &mut S, frame: &HostFrame) -> Result<()>
where
    S: futures::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    sink.send(Message::text(serde_json::to_string(frame)?)).await?;
    Ok(())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_connection`
Expected: 8 passed.

- [ ] **Step 5: Revert-probe the liveness fixes**

Remove `*backoff = cfg.reconnect_min;` from the `healthy` arm, and replace `cfg.connect_timeout` in `connect_once` with `Duration::from_secs(3600)`. Run `cargo test -p hennery-testkit --test host_connection`. Expected: `backoff_resets_after_a_healthy_connection_even_without_acks` and `a_collector_that_never_completes_the_handshake_is_retried` FAIL. Restore and re-run: 8 passed.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host/src/connection.rs crates/hennery-testkit/tests/host_connection.rs
git commit -m "feat(host): report open turns, dispatch park and close, bound connects and reset backoff when healthy"
```

---

### Task 7: Collector store: conflicts, teardown transitions, reconciliation

**Files:**
- Modify (rewrite): `crates/hennery-sessions/src/store.rs`
- Modify: `crates/hennery-sessions/Cargo.toml`
- Test: `crates/hennery-sessions/tests/store.rs`

**Interfaces:**
- Consumes (Task 1): `SessionBody::{SessionParked, SessionClosed, AdapterExited}`, `AttachedSession`.
- Schema (migration 2): `sessions.close_requested INTEGER NOT NULL DEFAULT 0`, and `turns.state TEXT NOT NULL DEFAULT 'sent'` with values `sent → started → ended | not_delivered`. Existing skeleton rows are backfilled from `outcome` / `turn_started` events.
- Produces: `SessionRow` gains `failure_reason: Option<String>` and `close_requested: bool`; `Store::turn_state(&str) -> Result<Option<String>>`.
- Produces operator records:
  - `Store::record_park_request(&str) -> Result<EventDto>` (`operator_parked`)
  - `Store::record_close_request(&str) -> Result<EventDto>` (`operator_closed` + `close_requested = 1`)
  - `Store::close_now(&str) -> Result<Vec<EventDto>>` (closed immediately, idempotent)
- Produces: `Store::reconcile_host(host_id: &str, attached: &[AttachedSession]) -> Result<Reconciliation>` with `pub struct Reconciliation { pub events: Vec<EventDto>, pub close: Vec<String> }`.
- Collector event kinds written: `conflict {seq, received}`, `start_not_delivered {}`, `turn_not_delivered {turn_id}`, `host_restarted {}`, `turn_ended_synthesized {turn_id, outcome: "interrupted"}`, `operator_parked {}`, `operator_closed {}`.
- Ingest rules:
  - `session_parked` makes an `active` session `parked`, or `closed` if a close was requested.
  - `session_closed` makes an `active` session `closed`.
  - `turn_started` reopens a turn that reconciliation had released (decision 2).

- [ ] **Step 1: Write the failing tests**

Add `[dev-dependencies]` with `tempfile = "3"` at the end of `crates/hennery-sessions/Cargo.toml`, then append to `crates/hennery-sessions/tests/store.rs`:

```rust
use hennery_proto::frames::{AttachedSession, ParkReason};

fn turn_started(turn: &str) -> SessionBody {
    SessionBody::TurnStarted {
        request_id: format!("req-{turn}"),
        turn_id: turn.into(),
    }
}

fn attached(session: &str, open_turn: Option<&str>) -> AttachedSession {
    AttachedSession {
        session_id: session.into(),
        last_seq: 0,
        open_turn_id: open_turn.map(str::to_string),
    }
}

fn prompt_text() -> Vec<serde_json::Value> {
    vec![json!({"type": "text", "text": "hi"})]
}

fn kinds(events: &[hennery_proto::rest::EventDto]) -> Vec<&str> {
    events.iter().map(|e| e.kind.as_str()).collect()
}

#[test]
fn a_same_seq_duplicate_with_a_different_body_is_kept_as_a_conflict() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.ingest("s1", 2, &update(1)).unwrap();
    let conflict = store.ingest("s1", 2, &update(2)).unwrap();
    assert_eq!(kinds(&conflict), ["conflict"]);
    assert_eq!(conflict[0].host_seq, None);
    assert_eq!(conflict[0].body["seq"], 2);
    assert_eq!(conflict[0].body["received"]["payload"]["n"], 2);
    // The committed row is untouched and an identical resend is still silent.
    assert!(store.ingest("s1", 2, &update(1)).unwrap().is_empty());
    let all = store.events("s1", 0, 100).unwrap();
    assert_eq!(
        all.iter().find(|e| e.host_seq == Some(2)).unwrap().body["payload"]["n"],
        1
    );
}

#[test]
fn session_parked_and_session_closed_detach_an_active_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("parked", None));

    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s2",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
            },
        )
        .unwrap();
    store.ingest("s2", 2, &SessionBody::SessionClosed).unwrap();
    assert_eq!(store.session("s2").unwrap().unwrap().lifecycle, "closed");
}

#[test]
fn a_park_that_overtakes_an_operator_close_closes_the_session() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.record_close_request("s1").unwrap();
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.close_requested), ("closed", false));
}

#[test]
fn close_now_closes_an_unattached_session_once() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store
        .ingest(
            "s1",
            2,
            &SessionBody::SessionParked {
                reason: ParkReason::Idle,
            },
        )
        .unwrap();
    assert_eq!(kinds(&store.close_now("s1").unwrap()), ["operator_closed"]);
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "closed");
    assert!(store.close_now("s1").unwrap().is_empty());
}

#[test]
fn reconcile_fails_a_start_the_host_never_received_and_leaves_a_running_start_alone() {
    let store = Store::open_in_memory().unwrap();
    store.create_session("lost", "h1", "fake", "/tmp").unwrap();
    store.create_session("pending", "h1", "fake", "/tmp").unwrap();
    store.create_session("other-host", "h2", "fake", "/tmp").unwrap();
    let r = store.reconcile_host("h1", &[attached("pending", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["start_not_delivered"]);
    let lost = store.session("lost").unwrap().unwrap();
    assert_eq!(
        (lost.lifecycle.as_str(), lost.failure_reason.as_deref()),
        ("failed", Some("start_not_delivered"))
    );
    assert_eq!(store.session("pending").unwrap().unwrap().lifecycle, "starting");
    assert_eq!(store.session("other-host").unwrap().unwrap().lifecycle, "starting");
}

#[test]
fn reconcile_parks_sessions_a_restarted_host_lost_and_interrupts_their_turn() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    let r = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(kinds(&r.events), ["host_restarted", "turn_ended_synthesized"]);
    assert_eq!(r.events[1].body, json!({"turn_id": "t1", "outcome": "interrupted"}));
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.open_turn_id.as_deref()), ("parked", None));
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("ended"));
    // Reconciling again changes nothing.
    assert_eq!(store.reconcile_host("h1", &[]).unwrap(), Default::default());
}

#[test]
fn reconcile_releases_a_prompt_that_never_reached_the_adapter() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    assert!(
        !store.open_turn("s1", "t2", &prompt_text()).unwrap(),
        "wedged until reconciled"
    );
    let r = store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["turn_not_delivered"]);
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("not_delivered"));
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!((s.lifecycle.as_str(), s.activity.as_deref()), ("active", Some("idle")));
    assert!(
        store.open_turn("s1", "t2", &prompt_text()).unwrap(),
        "the next prompt is accepted"
    );
}

#[test]
fn reconcile_keeps_a_turn_the_host_reports_open() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    let r = store.reconcile_host("h1", &[attached("s1", Some("t1"))]).unwrap();
    assert!(r.events.is_empty());
    assert_eq!(
        store.session("s1").unwrap().unwrap().open_turn_id.as_deref(),
        Some("t1")
    );
}

#[test]
fn reconcile_interrupts_a_started_turn_the_host_no_longer_reports() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
    let r = store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    assert_eq!(kinds(&r.events), ["turn_ended_synthesized"]);
    assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
}

#[test]
fn a_late_turn_started_reopens_a_turn_marked_not_delivered() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.reconcile_host("h1", &[attached("s1", None)]).unwrap();
    let created = store.ingest("s1", 2, &turn_started("t1")).unwrap();
    assert_eq!(kinds(&created), ["turn_started", "user_turn"]);
    let s = store.session("s1").unwrap().unwrap();
    assert_eq!(
        (s.open_turn_id.as_deref(), s.activity.as_deref()),
        (Some("t1"), Some("running"))
    );
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
}

#[test]
fn reconcile_asks_to_close_attached_sessions_the_operator_closed() {
    let store = Store::open_in_memory().unwrap();
    // Closed while its host was offline, still attached on the host.
    started(&store);
    store.close_now("s1").unwrap();
    // Close requested, delivery unknown, still attached.
    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s2",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
            },
        )
        .unwrap();
    store.record_close_request("s2").unwrap();
    // Close requested, and the host restarted meanwhile.
    store.create_session("s3", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s3",
            1,
            &SessionBody::SessionStarted {
                request_id: "r".into(),
                agent_session_id: "a".into(),
            },
        )
        .unwrap();
    store.record_close_request("s3").unwrap();

    let r = store
        .reconcile_host("h1", &[attached("s1", None), attached("s2", None)])
        .unwrap();
    assert_eq!(r.close, ["s1", "s2"]);
    assert_eq!(store.session("s3").unwrap().unwrap().lifecycle, "closed");
}

#[test]
fn the_teardown_migration_upgrades_skeleton_turns() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    {
        let store = Store::open(&db).unwrap();
        started(&store);
        store.open_turn("s1", "t1", &prompt_text()).unwrap();
        store.ingest("s1", 2, &turn_started("t1")).unwrap();
    }
    // Roll the file back to the walking skeleton's schema (version 1).
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "ALTER TABLE turns DROP COLUMN state;
             ALTER TABLE sessions DROP COLUMN close_requested;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert!(!store.session("s1").unwrap().unwrap().close_requested);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-sessions --test store`
Expected: compile error: no method `reconcile_host` / `record_close_request` / `close_now` / `turn_state`, and no fields `failure_reason` / `close_requested`.

- [ ] **Step 3: Implement**

Replace `crates/hennery-sessions/src/store.rs` with:

```rust
//! Collector session storage (ACP core §8). SQLite; writes are serialised by
//! the connection mutex (kernel §1's writer thread replaces it later).

use anyhow::Result;
use hennery_proto::frames::{AttachedSession, SessionBody};
use hennery_proto::rest::EventDto;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE sessions (
        id TEXT PRIMARY KEY,
        host_id TEXT NOT NULL,
        agent TEXT NOT NULL,
        cwd TEXT NOT NULL,
        agent_session_id TEXT,
        lifecycle TEXT NOT NULL,
        activity TEXT,
        failure_reason TEXT,
        open_turn_id TEXT,
        created_at TEXT NOT NULL,
        last_event_at TEXT NOT NULL);
    CREATE TABLE turns (
        turn_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        content TEXT NOT NULL,
        outcome TEXT,
        created_at TEXT NOT NULL);
    CREATE TABLE events (
        event_id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        host_seq INTEGER,
        kind TEXT NOT NULL,
        body TEXT NOT NULL,
        ts TEXT NOT NULL,
        UNIQUE(session_id, host_seq));
",
    // Teardown and reconciliation: a durable close intent (so a close whose
    // delivery is unknown is re-sent after the next handshake) and the turn
    // states reconciliation needs (sent → started → ended | not_delivered).
    "
    ALTER TABLE sessions ADD COLUMN close_requested INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE turns ADD COLUMN state TEXT NOT NULL DEFAULT 'sent';
    UPDATE turns SET state = 'ended' WHERE outcome IS NOT NULL;
    UPDATE turns SET state = 'started' WHERE outcome IS NULL AND EXISTS (
        SELECT 1 FROM events e
        WHERE e.kind = 'turn_started' AND json_extract(e.body, '$.turn_id') = turns.turn_id);
",
];

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub host_id: String,
    pub agent: String,
    pub cwd: String,
    pub lifecycle: String,
    pub activity: Option<String>,
    pub open_turn_id: Option<String>,
    pub failure_reason: Option<String>,
    /// The operator closed the session while it was attached and the host
    /// has not confirmed yet (ACP core §4.8).
    pub close_requested: bool,
}

/// What the collector did after a host's `resend_complete` (ACP core §5.1).
#[derive(Debug, Default, PartialEq)]
pub struct Reconciliation {
    /// Collector-originated events, in the order written.
    pub events: Vec<EventDto>,
    /// Attached sessions the operator has closed: send them `close_session`.
    pub close: Vec<String>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formatting of the current time")
}

/// Write a collector-originated event (`host_seq` NULL, ACP core §8).
fn collector_event(tx: &Transaction<'_>, session_id: &str, kind: &str, body: Value, ts: &str) -> Result<EventDto> {
    tx.execute(
        "INSERT INTO events(session_id, host_seq, kind, body, ts) VALUES (?1, NULL, ?2, ?3, ?4)",
        params![session_id, kind, body.to_string(), ts],
    )?;
    tx.execute(
        "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1",
        params![session_id, ts],
    )?;
    Ok(EventDto {
        event_id: tx.last_insert_rowid(),
        session_id: session_id.to_string(),
        host_seq: None,
        kind: kind.to_string(),
        body,
        ts: ts.to_string(),
    })
}

/// Close an open turn that the host will never end, as `interrupted`.
fn synthesize_turn_end(tx: &Transaction<'_>, session_id: &str, turn_id: &str, ts: &str) -> Result<EventDto> {
    tx.execute(
        "UPDATE turns SET state = 'ended', outcome = 'interrupted' WHERE turn_id = ?1",
        [turn_id],
    )?;
    tx.execute(
        "UPDATE sessions SET open_turn_id = NULL, activity = 'idle' WHERE id = ?1 AND open_turn_id = ?2",
        params![session_id, turn_id],
    )?;
    collector_event(
        tx,
        session_id,
        "turn_ended_synthesized",
        json!({ "turn_id": turn_id, "outcome": "interrupted" }),
        ts,
    )
}

/// Release an open turn whose prompt never reached the adapter.
fn turn_not_delivered(tx: &Transaction<'_>, session_id: &str, turn_id: &str, ts: &str) -> Result<EventDto> {
    tx.execute("UPDATE turns SET state = 'not_delivered' WHERE turn_id = ?1", [turn_id])?;
    tx.execute(
        "UPDATE sessions SET open_turn_id = NULL, activity = 'idle' WHERE id = ?1 AND open_turn_id = ?2",
        params![session_id, turn_id],
    )?;
    collector_event(tx, session_id, "turn_not_delivered", json!({ "turn_id": turn_id }), ts)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(hennery_kernel::db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(hennery_kernel::db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        hennery_kernel::db::migrate(&mut conn, MIGRATIONS)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store lock")
    }

    pub fn create_session(&self, id: &str, host_id: &str, agent: &str, cwd: &str) -> Result<()> {
        let ts = now();
        self.conn().execute(
            "INSERT INTO sessions(id, host_id, agent, cwd, lifecycle, created_at, last_event_at)
             VALUES (?1, ?2, ?3, ?4, 'starting', ?5, ?5)",
            params![id, host_id, agent, cwd, ts],
        )?;
        Ok(())
    }

    pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2 WHERE id = ?1",
            params![id, reason],
        )?;
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, host_id, agent, cwd, lifecycle, activity, open_turn_id, failure_reason, close_requested
                 FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        host_id: r.get(1)?,
                        agent: r.get(2)?,
                        cwd: r.get(3)?,
                        lifecycle: r.get(4)?,
                        activity: r.get(5)?,
                        open_turn_id: r.get(6)?,
                        failure_reason: r.get(7)?,
                        close_requested: r.get(8)?,
                    })
                },
            )
            .optional()?)
    }

    /// A turn's state: `sent`, `started`, `ended` or `not_delivered`.
    pub fn turn_state(&self, turn_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT state FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
            .optional()?)
    }

    /// Open a turn if the session is active and has none open. Returns false
    /// when a turn is already in flight (ACP core §4.4: one turn at a time).
    pub fn open_turn(&self, session_id: &str, turn_id: &str, content: &[Value]) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE sessions SET open_turn_id = ?2
             WHERE id = ?1 AND lifecycle = 'active' AND open_turn_id IS NULL",
            params![session_id, turn_id],
        )?;
        if changed == 1 {
            tx.execute(
                "INSERT INTO turns(turn_id, session_id, content, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![turn_id, session_id, serde_json::to_string(content)?, now()],
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    /// Undo `open_turn` after the host rejected the prompt.
    pub fn abandon_turn(&self, session_id: &str, turn_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sessions SET open_turn_id = NULL WHERE id = ?1 AND open_turn_id = ?2",
            params![session_id, turn_id],
        )?;
        tx.execute("DELETE FROM turns WHERE turn_id = ?1", [turn_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Record an operator park before `park_session` is sent.
    pub fn record_park_request(&self, session_id: &str) -> Result<EventDto> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let event = collector_event(&tx, session_id, "operator_parked", json!({}), &now())?;
        tx.commit()?;
        Ok(event)
    }

    /// Record an operator close of an attached session before
    /// `close_session` is sent. The intent is durable: if the host never
    /// confirms, the next handshake sends `close_session` again.
    pub fn record_close_request(&self, session_id: &str) -> Result<EventDto> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("UPDATE sessions SET close_requested = 1 WHERE id = ?1", [session_id])?;
        let event = collector_event(&tx, session_id, "operator_closed", json!({}), &now())?;
        tx.commit()?;
        Ok(event)
    }

    /// Close a session with no adapter the collector can reach (parked,
    /// failed, or its host offline) immediately (ACP core §4.8). Writes
    /// `operator_closed` unless a close request already recorded it.
    /// Idempotent: a closed session is left alone.
    pub fn close_now(&self, session_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, bool)> = tx
            .query_row(
                "SELECT lifecycle, close_requested FROM sessions WHERE id = ?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let mut events = Vec::new();
        if let Some((lifecycle, close_requested)) = row
            && lifecycle != "closed"
        {
            let ts = now();
            if !close_requested {
                events.push(collector_event(&tx, session_id, "operator_closed", json!({}), &ts)?);
            }
            tx.execute(
                "UPDATE sessions SET lifecycle = 'closed', activity = NULL, close_requested = 0 WHERE id = ?1",
                [session_id],
            )?;
        }
        tx.commit()?;
        Ok(events)
    }

    /// Highest committed host seq for a session (0 if none).
    pub fn committed_seq(&self, session_id: &str) -> Result<u64> {
        let v: Option<i64> = self.conn().query_row(
            "SELECT MAX(host_seq) FROM events WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        )?;
        Ok(v.unwrap_or(0) as u64)
    }

    /// Ingest one sequenced host frame. Idempotent on (session_id, seq): a
    /// duplicate with the same body is discarded; one with a different body
    /// is kept as a `conflict` event (ACP core §3.6). Returns the events it
    /// created, in order.
    pub fn ingest(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let kind = body_kind(body);
        let received = serde_json::to_value(body)?;
        let inserted = tx.execute(
            "INSERT INTO events(session_id, host_seq, kind, body, ts) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, host_seq) DO NOTHING",
            params![session_id, seq as i64, kind, received.to_string(), ts],
        )?;
        if inserted == 0 {
            let stored: String = tx.query_row(
                "SELECT body FROM events WHERE session_id = ?1 AND host_seq = ?2",
                params![session_id, seq as i64],
                |r| r.get(0),
            )?;
            // Structural comparison: key order is not stable across builds
            // (serde_json's `preserve_order` is feature-unified).
            let created = if serde_json::from_str::<Value>(&stored)? == received {
                Vec::new()
            } else {
                vec![collector_event(
                    &tx,
                    session_id,
                    "conflict",
                    json!({ "seq": seq, "received": received }),
                    &ts,
                )?]
            };
            tx.commit()?;
            return Ok(created);
        }
        let mut created = vec![EventDto {
            event_id: tx.last_insert_rowid(),
            session_id: session_id.to_string(),
            host_seq: Some(seq),
            kind: kind.to_string(),
            body: received,
            ts: ts.clone(),
        }];
        match body {
            SessionBody::SessionStarted { agent_session_id, .. } => {
                tx.execute(
                    "UPDATE sessions SET lifecycle = 'active', activity = 'idle', agent_session_id = ?2
                     WHERE id = ?1 AND lifecycle IN ('starting', 'failed')",
                    params![session_id, agent_session_id],
                )?;
            }
            SessionBody::StartFailed { code, .. } => {
                tx.execute(
                    "UPDATE sessions SET lifecycle = 'failed', failure_reason = ?2 WHERE id = ?1 AND lifecycle = 'starting'",
                    params![session_id, code],
                )?;
            }
            SessionBody::TurnStarted { turn_id, .. } => {
                // The fact wins over reconciliation: a turn marked
                // `not_delivered` whose `turn_started` arrives late is open
                // again (the adapter really has it).
                tx.execute(
                    "UPDATE sessions SET activity = 'running', open_turn_id = ?2
                     WHERE id = ?1 AND (open_turn_id IS NULL OR open_turn_id = ?2)",
                    params![session_id, turn_id],
                )?;
                tx.execute("UPDATE turns SET state = 'started' WHERE turn_id = ?1", [turn_id])?;
                // The user's turn is recorded only once the adapter has it
                // (ACP core §4.4), in seq order before the turn's updates.
                let content: Option<String> = tx
                    .query_row("SELECT content FROM turns WHERE turn_id = ?1", [turn_id], |r| r.get(0))
                    .optional()?;
                if let Some(content) = content {
                    let body = json!({ "turn_id": turn_id, "content": serde_json::from_str::<Value>(&content)? });
                    created.push(collector_event(&tx, session_id, "user_turn", body, &ts)?);
                }
            }
            SessionBody::TurnEnded { turn_id, outcome, .. } => {
                // Applied only to the open turn; a late duplicate for an
                // already-ended turn is stored but not applied, and (ACP core
                // §4.4) must never be pushed to a caller — only the store
                // knows whether the transition actually applied.
                let applied = tx.execute(
                    "UPDATE sessions SET open_turn_id = NULL, activity = 'idle' WHERE id = ?1 AND open_turn_id = ?2",
                    params![session_id, turn_id],
                )?;
                tx.execute(
                    "UPDATE turns SET state = 'ended', outcome = ?2 WHERE turn_id = ?1 AND outcome IS NULL",
                    params![turn_id, serde_json::to_value(outcome)?.as_str().unwrap_or_default()],
                )?;
                if applied == 0 {
                    created.clear();
                }
            }
            SessionBody::SessionParked { .. } => {
                // A park that overtakes an operator close ends the session
                // as the operator asked: closed.
                tx.execute(
                    "UPDATE sessions SET
                         lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                         activity = NULL, close_requested = 0
                     WHERE id = ?1 AND lifecycle = 'active'",
                    [session_id],
                )?;
            }
            SessionBody::SessionClosed => {
                tx.execute(
                    "UPDATE sessions SET lifecycle = 'closed', activity = NULL, close_requested = 0
                     WHERE id = ?1 AND lifecycle = 'active'",
                    [session_id],
                )?;
            }
            // Diagnostics only; the `session_parked` that follows detaches.
            SessionBody::AdapterExited { .. } | SessionBody::AcpUpdate { .. } => {}
        }
        tx.execute(
            "UPDATE sessions SET last_event_at = ?2 WHERE id = ?1",
            params![session_id, ts],
        )?;
        tx.commit()?;
        Ok(created)
    }

    /// Reconcile a host's sessions after its `resend_complete` (ACP core
    /// §5.1 step 4, §5.2). Everything the host had in its outbox has been
    /// ingested by now, so anything still unresolved never happened:
    ///
    /// - `starting`, not attached → `failed{start_not_delivered}`;
    /// - `active`, not attached → the host restarted: `host_restarted`,
    ///   parked (or closed, if the operator asked), its open turn ended;
    /// - an open turn the host does not report: `turn_not_delivered` if it
    ///   never started, `turn_ended_synthesized{interrupted}` if it did;
    /// - attached sessions the operator closed → returned in `close`.
    pub fn reconcile_host(&self, host_id: &str, attached: &[AttachedSession]) -> Result<Reconciliation> {
        let listed: HashMap<&str, &AttachedSession> = attached.iter().map(|a| (a.session_id.as_str(), a)).collect();
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        let rows: Vec<(String, String, Option<String>, bool)> = {
            let mut stmt = tx.prepare(
                "SELECT id, lifecycle, open_turn_id, close_requested FROM sessions
                 WHERE host_id = ?1 AND lifecycle IN ('starting', 'active', 'closed') ORDER BY id",
            )?;
            let rows = stmt.query_map([host_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Reconciliation::default();
        for (id, lifecycle, open_turn, close_requested) in rows {
            let host = listed.get(id.as_str());
            match (lifecycle.as_str(), host) {
                ("starting", None) => {
                    out.events
                        .push(collector_event(&tx, &id, "start_not_delivered", json!({}), &ts)?);
                    tx.execute(
                        "UPDATE sessions SET lifecycle = 'failed', failure_reason = 'start_not_delivered' WHERE id = ?1",
                        [&id],
                    )?;
                }
                ("active", _) => {
                    if host.is_none() {
                        out.events
                            .push(collector_event(&tx, &id, "host_restarted", json!({}), &ts)?);
                    }
                    let host_turn = host.and_then(|a| a.open_turn_id.as_deref());
                    if let Some(turn) = open_turn.as_deref()
                        && host_turn != Some(turn)
                    {
                        let state: Option<String> = tx
                            .query_row("SELECT state FROM turns WHERE turn_id = ?1", [turn], |r| r.get(0))
                            .optional()?;
                        out.events.push(match state.as_deref() {
                            Some("started") => synthesize_turn_end(&tx, &id, turn, &ts)?,
                            _ => turn_not_delivered(&tx, &id, turn, &ts)?,
                        });
                    }
                    if host.is_none() {
                        tx.execute(
                            "UPDATE sessions SET
                                 lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                                 activity = NULL, open_turn_id = NULL, close_requested = 0
                             WHERE id = ?1",
                            [&id],
                        )?;
                    } else if close_requested {
                        out.close.push(id);
                    }
                }
                ("closed", Some(_)) => out.close.push(id),
                _ => {}
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Events of one session with `event_id > after`, oldest first.
    pub fn events(&self, session_id: &str, after: i64, limit: u32) -> Result<Vec<EventDto>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT event_id, host_seq, kind, body, ts FROM events
             WHERE session_id = ?1 AND event_id > ?2 ORDER BY event_id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![session_id, after, limit], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (event_id, host_seq, kind, body, ts) = row?;
            out.push(EventDto {
                event_id,
                session_id: session_id.to_string(),
                host_seq: host_seq.map(|s| s as u64),
                kind,
                body: serde_json::from_str(&body)?,
                ts,
            });
        }
        Ok(out)
    }
}

fn body_kind(body: &SessionBody) -> &'static str {
    match body {
        SessionBody::SessionStarted { .. } => "session_started",
        SessionBody::StartFailed { .. } => "start_failed",
        SessionBody::TurnStarted { .. } => "turn_started",
        SessionBody::AcpUpdate { .. } => "acp_update",
        SessionBody::TurnEnded { .. } => "turn_ended",
        SessionBody::SessionParked { .. } => "session_parked",
        SessionBody::SessionClosed => "session_closed",
        SessionBody::AdapterExited { .. } => "adapter_exited",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamps_are_rfc3339_utc() {
        let ts = super::now();
        assert!(ts.ends_with('Z') && ts.as_bytes()[10] == b'T', "{ts}");
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-sessions`
Expected: `store` 19 passed; the unit test passes.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-sessions Cargo.lock
git commit -m "feat(sessions): record conflicts, apply park and close, reconcile a host after resend"
```

---

### Task 8: Collector hub and host socket: reconcile before any request

**Files:**
- Modify (rewrite): `crates/hennery-sessions/src/hub.rs`
- Modify: `crates/hennery-sessions/src/ws.rs`, `crates/hennery-testkit/tests/e2e.rs`
- Test: `crates/hennery-testkit/tests/reconcile.rs` (new)

**Interfaces:**
- Consumes (Task 7): `Store::reconcile_host`, `Reconciliation`.
- Produces:
  - `Hub::register(&str, UnboundedSender<CollectorFrame>) -> Option<Registration>`, with `pub struct Registration { pub conn_id: u64, pub kicked: CancellationToken }`. The return type was `Option<u64>`.
  - `Hub::mark_ready(host_id, conn_id)`.
  - `Hub::disconnect(host_id)`, which cancels `kicked`; the socket task then closes.
  - `Hub::request_for_session(host_id, request_id, session_id, frame, timeout)`, resolved by `Hub::resolve_session(session_id, fact)`.
- Behaviour:
  - `send`, `request` and `connected_hosts` see only **ready** hosts. A registered-but-unreconciled host answers `RequestError::NotConnected`.
  - A request that times out calls `disconnect` on its host (decision 1).
  - In `ws.rs`, the first `resend_complete` of a connection runs `reconcile_host` with that connection's `hello.attached_sessions`. It then publishes the events, sends `close_session` for every id in `close`, and marks the host ready. `session_parked` / `session_closed` resolve session waiters.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-testkit/tests/reconcile.rs`:

```rust
//! The collector's reconciliation (ACP core §3.4, §5.1, §5.2) against a
//! scripted host over a real WebSocket: the test plays the host frame by
//! frame, so it controls exactly what was delivered before a drop.

use futures::{SinkExt, StreamExt};
use hennery_kernel::auth::DevToken;
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::EventDto;
use hennery_sessions::{AppState, store::Store};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token";
const HOST: &str = "host-1";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn lifecycle(&self, session: &str) -> String {
        self.state.store.session(session).unwrap().unwrap().lifecycle
    }

    fn event_kinds(&self, session: &str) -> Vec<String> {
        let events: Vec<EventDto> = self.state.store.events(session, 0, 1000).unwrap();
        events.into_iter().map(|e| e.kind).collect()
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The test playing a host.
struct ScriptedHost {
    ws: Ws,
    seq: u64,
}

impl ScriptedHost {
    /// Connect and complete `hello` / `hello_ack`, without `resend_complete`.
    async fn hello(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let mut host = Self { ws, seq };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            token: TOKEN.into(),
            attached_sessions: attached,
        })
        .await;
        let ack = host.next().await;
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host
    }

    /// `hello` then `resend_complete`, and wait until the collector lists
    /// the host as connected (reconciled).
    async fn connect(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        let mut host = Self::hello(collector, attached, seq).await;
        host.send(&HostFrame::ResendComplete).await;
        wait_for("host ready", || async {
            collector
                .state
                .hub
                .connected_hosts()
                .contains(&HOST.to_string())
                .then_some(())
        })
        .await;
        host
    }

    async fn send(&mut self, frame: &HostFrame) {
        self.ws
            .send(Message::text(serde_json::to_string(frame).unwrap()))
            .await
            .unwrap();
    }

    /// Emit the next sequenced session frame.
    async fn emit(&mut self, session_id: &str, body: SessionBody) {
        self.seq += 1;
        let frame = HostFrame::Session {
            session_id: session_id.into(),
            seq: self.seq,
            body,
        };
        self.send(&frame).await;
    }

    /// The next collector frame that is not an `ack`.
    async fn next(&mut self) -> CollectorFrame {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match self.ws.next().await {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text).unwrap() {
                        CollectorFrame::Ack { .. } => {}
                        frame => return frame,
                    },
                    Some(Ok(_)) => {}
                    other => panic!("collector connection ended: {other:?}"),
                }
            }
        })
        .await
        .expect("a collector frame within 10s")
    }

    /// Drop the connection and wait until the collector has noticed.
    async fn drop_connection(self, collector: &Collector) {
        drop(self.ws);
        wait_for("host gone", || async {
            (!collector.state.hub.connected_hosts().contains(&HOST.to_string())).then_some(())
        })
        .await;
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

fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

fn attached(session_id: &str, last_seq: u64) -> AttachedSession {
    AttachedSession {
        session_id: session_id.into(),
        last_seq,
        open_turn_id: None,
    }
}

async fn post(c: &reqwest::Client, url: String, body: Value) -> (u16, Value) {
    // Bounded, so a request the collector wrongly sends to a silent host
    // fails the test instead of hanging it for the 90 s start timeout.
    let resp = c
        .post(url)
        .json(&body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

/// Start a session through the API, answering the host side by hand.
async fn started_session(collector: &Collector, host: &mut ScriptedHost) -> String {
    let c = client();
    let url = collector.url("/api/sessions");
    let call =
        tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
    let CollectorFrame::StartSession {
        request_id, session_id, ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    host.emit(
        &session_id,
        SessionBody::SessionStarted {
            request_id,
            agent_session_id: "agent-1".into(),
        },
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
    session_id
}

fn prompt_body() -> Value {
    json!({ "content": [{ "type": "text", "text": "hi" }] })
}

#[tokio::test]
async fn nothing_is_sent_to_a_host_before_its_reconciliation() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::hello(&collector, vec![], 0).await;
    // Connected, but its resend is not complete: not offered to clients...
    assert!(collector.state.hub.connected_hosts().is_empty());
    // ...and a start is refused rather than sent, so it cannot be mistaken
    // for a start lost on an earlier connection.
    let (status, body) = post(
        &client(),
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
    host.send(&HostFrame::ResendComplete).await;
    wait_for("host ready", || async {
        collector
            .state
            .hub
            .connected_hosts()
            .contains(&HOST.to_string())
            .then_some(())
    })
    .await;
    let session = started_session(&collector, &mut host).await;
    assert_eq!(collector.lifecycle(&session), "active");
}

#[tokio::test]
async fn a_start_lost_in_a_drop_fails_as_not_delivered_after_the_next_handshake() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client();
    let url = collector.url("/api/sessions");
    let call =
        tokio::spawn(async move { post(&c, url, json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" })).await });
    assert!(matches!(host.next().await, CollectorFrame::StartSession { .. }));
    host.drop_connection(&collector).await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 503);
    let session = body["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        collector.lifecycle(&session),
        "starting",
        "not failed before reconciliation"
    );

    // The host comes back without the session: the start never happened.
    let _host = ScriptedHost::connect(&collector, vec![], 0).await;
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.failure_reason.as_deref()),
        ("failed", Some("start_not_delivered"))
    );
}

#[tokio::test]
async fn a_prompt_lost_in_a_drop_is_released_and_the_next_prompt_runs() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));

    let c = client();
    let url = prompt_url.clone();
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    assert!(matches!(host.next().await, CollectorFrame::Prompt { .. }));
    let seq = host.seq;
    host.drop_connection(&collector).await; // the prompt never reached the adapter
    assert_eq!(call.await.unwrap().0, 503);
    // Still wedged until the host is back: one turn at a time.
    assert_eq!(post(&client(), prompt_url.clone(), prompt_body()).await.0, 409);

    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    assert!(
        collector
            .event_kinds(&session)
            .contains(&"turn_not_delivered".to_string())
    );
    let c = client();
    let url = prompt_url.clone();
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    let CollectorFrame::Prompt {
        request_id, turn_id, ..
    } = host.next().await
    else {
        panic!("expected the next prompt");
    };
    host.emit(&session, SessionBody::TurnStarted { request_id, turn_id })
        .await;
    assert_eq!(call.await.unwrap().0, 202);
}

#[tokio::test]
async fn a_restarted_host_parks_its_sessions_and_interrupts_the_open_turn_only_after_resend() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client();
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let call = tokio::spawn(async move { post(&c, url, prompt_body()).await });
    let CollectorFrame::Prompt {
        request_id, turn_id, ..
    } = host.next().await
    else {
        panic!("expected a prompt");
    };
    host.emit(&session, SessionBody::TurnStarted { request_id, turn_id })
        .await;
    assert_eq!(call.await.unwrap().0, 202);
    host.drop_connection(&collector).await;

    // The restarted host lists nothing. Before its resend completes the
    // collector must not act: the turn's end may still be in the outbox.
    let mut host = ScriptedHost::hello(&collector, vec![], 0).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(collector.lifecycle(&session), "active");
    host.send(&HostFrame::ResendComplete).await;
    wait_for("parked", || async {
        (collector.lifecycle(&session) == "parked").then_some(())
    })
    .await;
    let kinds = collector.event_kinds(&session);
    assert!(
        kinds.ends_with(&["host_restarted".to_string(), "turn_ended_synthesized".to_string()]),
        "{kinds:?}"
    );
    let (status, body) = post(
        &client(),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

#[tokio::test]
async fn a_request_that_times_out_on_a_live_connection_drops_it() {
    let collector = Collector::start().await;
    let _host = ScriptedHost::connect(&collector, vec![], 0).await;
    let request = CollectorFrame::StartSession {
        request_id: "r-timeout".into(),
        session_id: "s-timeout".into(),
        agent: "fake".into(),
        cwd: "/tmp".into(),
    };
    let outcome = collector
        .state
        .hub
        .request(HOST, "r-timeout", request, Duration::from_millis(100))
        .await;
    assert_eq!(outcome, Err(hennery_sessions::hub::RequestError::DeliveryUnknown));
    wait_for("connection dropped", || async {
        (!collector.state.hub.connected_hosts().contains(&HOST.to_string())).then_some(())
    })
    .await;
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test reconcile`
Expected: `nothing_is_sent_to_a_host_before_its_reconciliation` fails (the host is listed before `resend_complete`); the drop/restart tests fail on their reconciliation assertions (`starting` / wedged 409 / still `active`); the timeout test fails (connection not dropped).

- [ ] **Step 3: Rewrite the hub**

Replace `crates/hennery-sessions/src/hub.rs` with:

```rust
//! Connected hosts and in-flight collector→host requests.

use hennery_proto::frames::{CollectorFrame, SessionBody};
use hennery_proto::rest::EventDto;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Why a request produced no fact.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    /// The host is not connected, or is connected but not yet reconciled.
    NotConnected,
    /// The host rejected the request; nothing happened.
    Rejected { code: String, message: String },
    /// The connection dropped or the timeout passed: the request may or may
    /// not have been delivered. Reconciled after the host's next handshake.
    DeliveryUnknown,
}

struct Waiter {
    host_id: String,
    /// Set for requests completed by a fact that names only the session
    /// (`session_parked`, `session_closed`), not the request (§3.2).
    session_id: Option<String>,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}

struct HostConn {
    conn_id: u64,
    tx: mpsc::UnboundedSender<CollectorFrame>,
    /// Requests are sent only after the post-`resend_complete`
    /// reconciliation, so nothing sent on this connection is mistaken for
    /// something lost on the previous one (ACP core §5.1).
    ready: bool,
    kicked: CancellationToken,
}

/// A registered host connection.
pub struct Registration {
    pub conn_id: u64,
    /// Cancelled when the hub drops this connection (a request timed out on
    /// it); the socket task must then close the socket.
    pub kicked: CancellationToken,
}

pub struct Hub {
    next_conn: AtomicU64,
    hosts: Mutex<HashMap<String, HostConn>>,
    waiters: Mutex<HashMap<String, Waiter>>,
    events: broadcast::Sender<EventDto>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            next_conn: AtomicU64::new(1),
            hosts: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            events: broadcast::channel(1024).0,
        }
    }

    /// Register a host connection (not yet ready). A second live connection
    /// for the same host id is refused, never allowed to supersede the
    /// first silently.
    pub fn register(&self, host_id: &str, tx: mpsc::UnboundedSender<CollectorFrame>) -> Option<Registration> {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| !h.tx.is_closed()) {
            return None;
        }
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let kicked = CancellationToken::new();
        hosts.insert(
            host_id.to_string(),
            HostConn {
                conn_id,
                tx,
                ready: false,
                kicked: kicked.clone(),
            },
        );
        Some(Registration { conn_id, kicked })
    }

    /// Reconciliation for this connection is done: requests may flow.
    pub fn mark_ready(&self, host_id: &str, conn_id: u64) {
        if let Some(h) = self.hosts.lock().expect("hosts lock").get_mut(host_id)
            && h.conn_id == conn_id
        {
            h.ready = true;
        }
    }

    /// Drop a connection and fail its in-flight requests as delivery-unknown.
    pub fn unregister(&self, host_id: &str, conn_id: u64) {
        let mut hosts = self.hosts.lock().expect("hosts lock");
        if hosts.get(host_id).is_some_and(|h| h.conn_id == conn_id) {
            hosts.remove(host_id);
        }
        drop(hosts);
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| w.host_id == host_id)
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(w) = waiters.remove(&id) {
                let _ = w.tx.send(Err(RequestError::DeliveryUnknown));
            }
        }
    }

    /// Close a host's connection from the collector side. The host
    /// reconnects, and the handshake reconciles whatever was in doubt.
    pub fn disconnect(&self, host_id: &str) {
        if let Some(h) = self.hosts.lock().expect("hosts lock").get(host_id) {
            h.kicked.cancel();
        }
    }

    /// Hosts that are connected and reconciled, sorted.
    pub fn connected_hosts(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .hosts
            .lock()
            .expect("hosts lock")
            .iter()
            .filter(|(_, h)| h.ready)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Send a frame to a ready host without waiting for anything.
    pub fn send(&self, host_id: &str, frame: CollectorFrame) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.ready && h.tx.send(frame).is_ok())
    }

    /// Send a request and wait until the outboxed fact carrying `request_id`
    /// is ingested (`resolve`), the host rejects it (`reject`), the
    /// connection drops, or `timeout` passes.
    pub async fn request(
        &self,
        host_id: &str,
        request_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        self.wait(host_id, request_id, None, frame, timeout).await
    }

    /// Like `request`, for requests completed by a fact that names only the
    /// session (`resolve_session`). Rejections still match `request_id`.
    pub async fn request_for_session(
        &self,
        host_id: &str,
        request_id: &str,
        session_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        self.wait(host_id, request_id, Some(session_id.to_string()), frame, timeout)
            .await
    }

    async fn wait(
        &self,
        host_id: &str,
        request_id: &str,
        session_id: Option<String>,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().expect("waiters lock").insert(
            request_id.to_string(),
            Waiter {
                host_id: host_id.to_string(),
                session_id,
                tx,
            },
        );
        if !self.send(host_id, frame) {
            self.waiters.lock().expect("waiters lock").remove(request_id);
            return Err(RequestError::NotConnected);
        }
        let result = tokio::time::timeout(timeout, rx).await;
        self.waiters.lock().expect("waiters lock").remove(request_id);
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(RequestError::DeliveryUnknown),
            Err(_elapsed) => {
                // Every timeout is at least the read deadline, so a live
                // connection that produced neither the fact nor a rejection
                // is not to be trusted: drop it, and the next handshake
                // reconciles this request (ACP core §3.4).
                tracing::warn!(%host_id, %request_id, "request timed out; dropping the host connection");
                self.disconnect(host_id);
                Err(RequestError::DeliveryUnknown)
            }
        }
    }

    pub fn resolve(&self, request_id: &str, fact: SessionBody) {
        if let Some(w) = self.waiters.lock().expect("waiters lock").remove(request_id) {
            let _ = w.tx.send(Ok(fact));
        }
    }

    /// Resolve every waiter registered with `request_for_session` for
    /// `session_id`.
    pub fn resolve_session(&self, session_id: &str, fact: SessionBody) {
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| w.session_id.as_deref() == Some(session_id))
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            if let Some(w) = waiters.remove(&id) {
                let _ = w.tx.send(Ok(fact.clone()));
            }
        }
    }

    pub fn reject(&self, request_id: &str, code: String, message: String) {
        if let Some(w) = self.waiters.lock().expect("waiters lock").remove(request_id) {
            let _ = w.tx.send(Err(RequestError::Rejected { code, message }));
        }
    }

    pub fn publish(&self, event: EventDto) {
        let _ = self.events.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventDto> {
        self.events.subscribe()
    }
}
```

- [ ] **Step 4: Reconcile in the host socket**

Replace `crates/hennery-sessions/src/ws.rs` with:

```rust
//! The host WebSocket endpoint (ACP core §3, §5).

use crate::AppState;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use hennery_proto::frames::{CollectorFrame, HostFrame, SessionBody};
use hennery_proto::{PROTOCOL_VERSION, protocol_major};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::sync::mpsc;

const MAX_FRAME: usize = 32 << 20;
const PING_INTERVAL: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(45);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

pub fn router(state: AppState) -> Router {
    Router::new().route("/api/hosts/ws", get(upgrade)).with_state(state)
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| serve(socket, state))
}

fn text(frame: &CollectorFrame) -> Message {
    Message::Text(serde_json::to_string(frame).expect("frame serializes").into())
}

async fn serve(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();

    // 1. hello: first frame, authenticated.
    let hello = match tokio::time::timeout(HELLO_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<HostFrame>(&t).ok(),
        _ => None,
    };
    let Some(HostFrame::Hello {
        protocol_version,
        host_id,
        token,
        attached_sessions,
        ..
    }) = hello
    else {
        return;
    };
    let reject = |code: &str, message: &str| CollectorFrame::HelloError {
        code: code.into(),
        message: message.into(),
    };
    if protocol_major(&protocol_version) != protocol_major(PROTOCOL_VERSION) {
        let _ = sink
            .send(text(&reject("incompatible", "unsupported protocol major")))
            .await;
        return;
    }
    if !state.token.matches(&token) {
        // The dev token stands in for ACP core §3.3's proof of identity.
        let _ = sink.send(text(&reject("bad_proof", "invalid host credential"))).await;
        return;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<CollectorFrame>();
    let Some(registration) = state.hub.register(&host_id, tx.clone()) else {
        let _ = sink
            .send(text(&reject(
                "already_connected",
                "another connection for this host is live",
            )))
            .await;
        return;
    };

    let mut committed = BTreeMap::new();
    for a in &attached_sessions {
        committed.insert(
            a.session_id.clone(),
            state.store.committed_seq(&a.session_id).unwrap_or(0),
        );
    }
    let ack = CollectorFrame::HelloAck {
        protocol_version: PROTOCOL_VERSION.into(),
        collector_version: env!("CARGO_PKG_VERSION").into(),
        committed,
    };
    let conn_id = registration.conn_id;
    if sink.send(text(&ack)).await.is_err() {
        state.hub.unregister(&host_id, conn_id);
        return;
    }
    tracing::info!(%host_id, "host connected");

    // 2. writer: frames for this host plus keepalive pings.
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        loop {
            tokio::select! {
                frame = rx.recv() => match frame {
                    Some(frame) => if sink.send(text(&frame)).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Default::default())).await.is_err() { break },
            }
        }
    });

    // 3. reader. Requests reach this host only once its resend is complete
    // and reconciled (`mark_ready`).
    let mut reconciled = false;
    loop {
        let next = tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = registration.kicked.cancelled() => break,
            next = tokio::time::timeout(READ_TIMEOUT, stream.next()) => next,
        };
        let msg = match next {
            Ok(Some(Ok(msg))) => msg,
            _ => break,
        };
        let Message::Text(t) = msg else {
            if matches!(msg, Message::Close(_)) {
                break;
            }
            continue;
        };
        let frame = match serde_json::from_str::<HostFrame>(&t) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(%host_id, error = %err, "ignoring unknown or invalid frame");
                continue;
            }
        };
        match frame {
            HostFrame::Session { session_id, seq, body } => {
                // A host may only write to its own sessions. Acks are
                // cumulative and the host prunes its outbox on ack, so any
                // store error here (lookup or ingest) must drop the
                // connection rather than silently skip the frame: acking a
                // later frame would tell the host this one is safe to
                // discard forever (ACP core §3.3, §5).
                match state.store.session(&session_id) {
                    Ok(Some(row)) if row.host_id == host_id => {}
                    Ok(_) => {
                        tracing::warn!(%host_id, %session_id, "frame for a session this host does not own");
                        continue;
                    }
                    Err(err) => {
                        tracing::error!(%host_id, %session_id, error = %err, "store lookup failed; dropping connection without acking");
                        break;
                    }
                }
                match state.store.ingest(&session_id, seq, &body) {
                    Ok(created) => {
                        for event in created {
                            state.hub.publish(event);
                        }
                        match &body {
                            SessionBody::SessionStarted { request_id, .. }
                            | SessionBody::TurnStarted { request_id, .. } => {
                                state.hub.resolve(request_id, body.clone());
                            }
                            SessionBody::StartFailed {
                                request_id,
                                code,
                                message,
                            } => {
                                state.hub.reject(request_id, code.clone(), message.clone());
                            }
                            // Park and close are completed by facts that
                            // name only the session (ACP core §3.2).
                            SessionBody::SessionParked { .. } | SessionBody::SessionClosed => {
                                state.hub.resolve_session(&session_id, body.clone());
                            }
                            _ => {}
                        }
                        let _ = tx.send(CollectorFrame::Ack {
                            session_id,
                            ack_seq: seq,
                        });
                    }
                    Err(err) => {
                        tracing::error!(%host_id, error = %err, "ingest failed; dropping connection without acking");
                        break;
                    }
                }
            }
            HostFrame::Error {
                request_id,
                code,
                message,
            } => state.hub.reject(&request_id, code, message),
            HostFrame::ResendComplete if !reconciled => {
                // Everything the host had in its outbox is ingested: only now
                // is anything still unresolved known to be lost (ACP core
                // §5.1 step 4).
                match state.store.reconcile_host(&host_id, &attached_sessions) {
                    Ok(done) => {
                        for event in done.events {
                            state.hub.publish(event);
                        }
                        for session_id in done.close {
                            let _ = tx.send(CollectorFrame::CloseSession {
                                request_id: uuid::Uuid::now_v7().to_string(),
                                session_id,
                            });
                        }
                        reconciled = true;
                        state.hub.mark_ready(&host_id, conn_id);
                        tracing::info!(%host_id, "host reconciled");
                    }
                    Err(err) => {
                        tracing::error!(%host_id, error = %err, "reconciliation failed; dropping connection");
                        break;
                    }
                }
            }
            HostFrame::ResendComplete => tracing::warn!(%host_id, "ignoring repeated resend_complete"),
            HostFrame::Hello { .. } => tracing::warn!(%host_id, "ignoring repeated hello"),
        }
    }

    writer.abort();
    state.hub.unregister(&host_id, conn_id);
    tracing::info!(%host_id, "host disconnected");
}
```

In `crates/hennery-testkit/tests/e2e.rs`, `a_start_with_unknown_delivery_reports_503_with_the_session_id` registers a host by hand. Its `let conn_id = …register(…).expect(…);` line becomes:

```rust
    let conn_id = collector
        .state
        .hub
        .register("host-1", tx)
        .expect("register fake host")
        .conn_id;
    collector.state.hub.mark_ready("host-1", conn_id);
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test reconcile && cargo test --workspace`
Expected: `reconcile` 5 passed; the whole workspace passes (existing e2e tests wait on `/api/hosts`, which now lists a host once it is reconciled).

- [ ] **Step 6: Revert-probe the readiness gate**

In `Hub::send`, change `.is_some_and(|h| h.ready && h.tx.send(frame).is_ok())` to `.is_some_and(|h| h.tx.send(frame).is_ok())`. Run `cargo test -p hennery-testkit --test reconcile nothing_is_sent`. Expected: FAIL after about 15 s (the start is sent to a host that never answers). Restore and re-run: PASS.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-sessions crates/hennery-testkit/tests/reconcile.rs crates/hennery-testkit/tests/e2e.rs
git commit -m "feat(sessions): reconcile a host after resend_complete before sending it requests"
```

---

### Task 9: Park and close API, end-to-end teardown scenarios

**Files:**
- Modify: `crates/hennery-sessions/src/api.rs`, `crates/hennery-sessions/src/hub.rs`
- Test: `crates/hennery-testkit/tests/e2e.rs`

**Interfaces:**
- Consumes: `Hub::request_for_session`, `Store::{record_park_request, record_close_request, close_now}`, `LifecycleResponse`.
- Produces:
  - `Hub::is_ready(&str) -> bool`.
  - `POST /api/sessions/{id}/park`: 202 `LifecycleResponse{lifecycle: "parked"}`; 409 `not_attached` unless the session is `active` on a reconciled host.
  - `POST /api/sessions/{id}/close`: 202 `LifecycleResponse{lifecycle: "closed"}`. It waits for `session_closed` when attached and closes immediately otherwise; it returns 409 `starting` while a start is running on a reconciled host, and 503 `delivery_unknown` if the close's delivery is unknown (it stays requested and is re-sent after the next handshake).
  - A host rejection `turn_in_progress` maps to 409.
- Test helper change: `start_host(…)` now returns the host's `JoinHandle<()>` and delegates to a new `start_host_with(collector, data_dir, fake: AgentCommand)`. Aborting that handle is an in-process host restart.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/e2e.rs`, replace the whole `start_host` function with:

```rust
fn start_host(collector: SocketAddr, data_dir: &Path, script: &FakeScript) -> tokio::task::JoinHandle<()> {
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(script).unwrap()));
    start_host_with(collector, data_dir, fake)
}

/// A host whose `fake` agent is `fake` (plus a `broken` one that cannot
/// spawn). Aborting the returned task is a host restart: its actors end and
/// kill their adapters.
fn start_host_with(collector: SocketAddr, data_dir: &Path, fake: AgentCommand) -> tokio::task::JoinHandle<()> {
    let mut cfg = HostConfig::new(
        format!("ws://{collector}/api/hosts/ws"),
        "host-1",
        TOKEN,
        data_dir.to_path_buf(),
    );
    cfg.reconnect_min = Duration::from_millis(100);
    cfg.reconnect_max = Duration::from_millis(500);
    cfg.agents.insert("fake".into(), fake);
    cfg.agents.insert(
        "broken".into(),
        AgentCommand::parse("/nonexistent/hennery-test-adapter").unwrap(),
    );
    tokio::spawn(async move {
        hennery_host::run(cfg).await.unwrap();
    })
}
```

Then append:

```rust
async fn lifecycle_is(collector: &Collector, session: &str, want: &str) {
    wait_for(&format!("lifecycle {want}"), || async {
        let row = collector.state.store.session(session).unwrap().unwrap();
        (row.lifecycle == want).then_some(())
    })
    .await;
}

fn of_kind<'a>(events: &'a [EventDto], kind: &str) -> Vec<&'a EventDto> {
    events.iter().filter(|e| e.kind == kind).collect()
}

async fn post_json(c: &reqwest::Client, url: String, body: Value) -> (u16, Value) {
    let resp = c.post(url).json(&body).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

fn slow_script(chunks: usize) -> FakeScript {
    FakeScript {
        chunks: (1..=chunks).map(|n| format!("{n}.")).collect(),
        chunk_delay_ms: 200,
        ..FakeScript::default()
    }
}

fn pid_from(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

async fn wait_dead(pid: i32) {
    wait_for("adapter subtree killed", || async {
        (!hennery_testkit::pid_alive(pid)).then_some(())
    })
    .await;
}

#[tokio::test]
async fn an_adapter_crash_mid_turn_parks_the_session_with_a_scrubbed_stderr_tail() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        chunks: vec!["1.".into(), "2.".into(), "3.".into()],
        chunk_delay_ms: 100,
        exit_after_chunks: Some(1),
        stderr_lines: vec!["using key sk-live-abcdefghijk".into()],
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(
        post_json(&c, prompt_url.clone(), json!({ "content": text("go") }))
            .await
            .0,
        202
    );

    lifecycle_is(&collector, &session, "parked").await;
    let evs = events(&c, &collector, &session).await;
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].body["outcome"], "interrupted");
    let exited = of_kind(&evs, "adapter_exited");
    assert_eq!(exited.len(), 1);
    let tail = exited[0].body["stderr_tail"].as_str().unwrap();
    assert!(
        tail.contains("sk-[redacted]") && !tail.contains("abcdefghijk"),
        "{tail}"
    );
    assert_eq!(of_kind(&evs, "session_parked")[0].body["reason"], "adapter_exited");
    let (status, body) = post_json(&c, prompt_url, json!({ "content": text("again") })).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

#[tokio::test]
async fn park_then_close_through_the_api_kill_the_adapters_whole_group() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..slow_script(20)
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let grandchild = wait_for("grandchild pid", || async { pid_from(&pid_file) }).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(post_json(&c, prompt_url, json!({ "content": text("go") })).await.0, 202);

    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/park")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("parked")), "{body}");
    wait_dead(grandchild).await;
    let evs = events(&c, &collector, &session).await;
    assert_eq!(turn_ends(&evs)[0].body["outcome"], "interrupted");
    assert_eq!(of_kind(&evs, "session_parked")[0].body["reason"], "operator");

    // A parked session closes at once, collector-side.
    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/close")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("closed")), "{body}");
    let kinds: Vec<String> = events(&c, &collector, &session)
        .await
        .into_iter()
        .map(|e| e.kind)
        .collect();
    let tail: Vec<&str> = kinds.iter().rev().take(1).map(String::as_str).collect();
    assert_eq!(tail, ["operator_closed"], "{kinds:?}");
    assert!(kinds.contains(&"operator_parked".to_string()), "{kinds:?}");
}

#[tokio::test]
async fn closing_an_attached_session_waits_for_the_host_to_close_it() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let (status, body) = post_json(&c, collector.url(&format!("/api/sessions/{session}/close")), json!({})).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("closed")), "{body}");
    let kinds: Vec<String> = events(&c, &collector, &session)
        .await
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.ends_with(&["operator_closed".to_string(), "session_closed".to_string()]),
        "{kinds:?}"
    );
}

#[tokio::test]
async fn a_host_restart_mid_turn_parks_and_interrupts_without_respawning() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let spawns = dir.path().join("spawns");
    let script = slow_script(20);
    // Counts adapter launches, then becomes the fake adapter.
    let counting = AgentCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            format!(
                "echo spawned >> {}; exec {}",
                spawns.display(),
                env!("CARGO_BIN_EXE_hennery-fake-acp")
            ),
        ],
        env: vec![(SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap())],
    };
    let host = start_host_with(collector.addr, &dir.path().join("host"), counting.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(
        post_json(&c, prompt_url.clone(), json!({ "content": text("go") }))
            .await
            .0,
        202
    );
    wait_for("first chunk", || async {
        (!agent_text(&events(&c, &collector, &session).await).is_empty()).then_some(())
    })
    .await;

    host.abort(); // the host process dies with its adapters
    let _ = host.await;
    start_host_with(collector.addr, &dir.path().join("host"), counting);

    lifecycle_is(&collector, &session, "parked").await;
    let evs = events(&c, &collector, &session).await;
    assert_eq!(of_kind(&evs, "host_restarted").len(), 1);
    let synthesized = of_kind(&evs, "turn_ended_synthesized");
    assert_eq!(synthesized.len(), 1);
    assert_eq!(synthesized[0].body["outcome"], "interrupted");
    assert!(turn_ends(&evs).is_empty(), "the dead host cannot have ended the turn");
    let (status, body) = post_json(&c, prompt_url, json!({ "content": text("again") })).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        std::fs::read_to_string(&spawns).unwrap().lines().count(),
        1,
        "no eager re-spawn"
    );
}

#[tokio::test]
async fn a_dropped_connection_mid_turn_parks_nothing_and_loses_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &slow_script(6));
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
    assert_eq!(post_json(&c, prompt_url, json!({ "content": text("go") })).await.0, 202);
    wait_for("first chunk", || async {
        (!agent_text(&events(&c, &collector, &session).await).is_empty()).then_some(())
    })
    .await;

    collector.state.hub.disconnect("host-1");
    let evs = wait_for("turn end after the drop", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(agent_text(&evs), "1.2.3.4.5.6.");
    assert_eq!(turn_ends(&evs)[0].body["outcome"], "completed");
    assert!(of_kind(&evs, "host_restarted").is_empty());
    assert!(of_kind(&evs, "turn_ended_synthesized").is_empty());
    assert_eq!(
        collector.state.store.session(&session).unwrap().unwrap().lifecycle,
        "active"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test e2e`
Expected: the park and close tests fail with 404 (no route). The crash, restart and drop scenarios already pass: they pin Tasks 4–8 end to end.

- [ ] **Step 3: Implement the endpoints**

In `crates/hennery-sessions/src/hub.rs`, add before `send`:

```rust
    /// The host is connected and reconciled.
    pub fn is_ready(&self, host_id: &str) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.ready)
    }
```

In `crates/hennery-sessions/src/api.rs`:
- the `hennery_proto::rest` import gains `LifecycleResponse`:
  `ApiError, EventDto, LifecycleResponse, PromptRequest, PromptResponse, StartSessionRequest, StartSessionResponse,`
- after `PROMPT_TIMEOUT`, add:
  ```rust
  /// `park_session` / `close_session` (ACP core §3.4).
  const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(60);
  ```
- in `router`, after the `/prompt` route, add:
  ```rust
          .route("/api/sessions/{id}/park", post(park))
          .route("/api/sessions/{id}/close", post(close))
  ```
- in `request_failed`, the `"not_attached" => StatusCode::CONFLICT,` arm becomes `"not_attached" | "turn_in_progress" => StatusCode::CONFLICT,`
- add before `#[derive(Deserialize)] struct EventsQuery`:

```rust
fn lifecycle_response(state: &AppState, id: &str) -> Response {
    match state.store.session(id) {
        Ok(Some(s)) => (
            StatusCode::ACCEPTED,
            Json(LifecycleResponse {
                session_id: s.id,
                lifecycle: s.lifecycle,
            }),
        )
            .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}

/// Close collector-side: the session has no adapter the collector can
/// reach (ACP core §4.8).
fn close_unattached(state: &AppState, id: &str) -> Response {
    match state.store.close_now(id) {
        Ok(events) => {
            for event in events {
                state.hub.publish(event);
            }
            lifecycle_response(state, id)
        }
        Err(err) => internal(err),
    }
}

/// Explicit park of an attached session: 202 with the lifecycle once the
/// host's `session_parked` is ingested.
async fn park(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" || !state.hub.is_ready(&session.host_id) {
        return error(StatusCode::CONFLICT, "not_attached", "the session is not attached");
    }
    match state.store.record_park_request(&id) {
        Ok(event) => state.hub.publish(event),
        Err(err) => return internal(err),
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::ParkSession {
        request_id: request_id.clone(),
        session_id: id.clone(),
    };
    match state
        .hub
        .request_for_session(&session.host_id, &request_id, &id, frame, TEARDOWN_TIMEOUT)
        .await
    {
        Ok(_) => lifecycle_response(&state, &id),
        Err(err) => request_failed(err),
    }
}

/// Close: attached sessions are closed by their host (`session_closed`);
/// anything else is closed immediately. A close whose delivery is unknown
/// stays requested and is re-sent after the host's next handshake.
async fn close(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    let reachable = state.hub.is_ready(&session.host_id);
    match session.lifecycle.as_str() {
        "closed" => return lifecycle_response(&state, &id),
        "starting" if reachable => {
            return error(
                StatusCode::CONFLICT,
                "starting",
                "the session is starting; close it once the start settles",
            );
        }
        "active" if reachable => {}
        _ => return close_unattached(&state, &id),
    }
    match state.store.record_close_request(&id) {
        Ok(event) => state.hub.publish(event),
        Err(err) => return internal(err),
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::CloseSession {
        request_id: request_id.clone(),
        session_id: id.clone(),
    };
    match state
        .hub
        .request_for_session(&session.host_id, &request_id, &id, frame, TEARDOWN_TIMEOUT)
        .await
    {
        // `session_closed`, or a `session_parked` that overtook the close
        // (ingest turns that into `closed`, since a close was requested).
        Ok(_) => lifecycle_response(&state, &id),
        // The host no longer has it (or went away): nothing left to stop.
        Err(RequestError::Rejected { code, .. }) if code == "not_attached" => close_unattached(&state, &id),
        Err(RequestError::NotConnected) => close_unattached(&state, &id),
        Err(err) => request_failed(err),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test e2e` (run it three times; it must be stable).
Expected: 11 passed each time.

- [ ] **Step 5: Full verification**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo run -p hennery-proto --bin gen --locked -- --check
```
Expected: all clean; 98 tests pass in the workspace (51 before this plan).

- [ ] **Step 6: Commit**

```bash
git add crates/hennery-sessions crates/hennery-testkit/tests/e2e.rs
git commit -m "feat(sessions): add park and close endpoints and end-to-end teardown scenarios"
```

---

## After this plan

**Plan B, resume.** It builds on this plan's actor and reconciliation:
- `resume_session` (§3.3), with `session/load` and replay suppression in the actor (§4.5, unknown kinds counted in a `host_note`).
- `committed_seq` in `start_session` / `resume_session`, and the fast-forward **before** the resumed session's first enqueue. This is the skeleton's "hold enqueue" obligation.
- Re-applying stored model/axes/mode (§4.3).
- `-32002` → `failed{agent_has_no_record}` and `-32000` → `failed{agent_not_logged_in}`.
- `POST …/resume` with 409 while `starting`/`active` (scenario 11: concurrent resume ×2).
- Host-offline presumed park and `reattached` (§5.3, scenario 8).
- `cancel_turn` (§3.3, `turn_ended{cancelled}`).
- `hello.capabilities`, and gating park on it.
- `GET /api/sessions/{id}` detail with the open turn.
- **Clear a stale open turn on detach.** A host's `not_attached` answer to a prompt is not outboxed. If it is lost, the collector can ingest `session_parked` / `session_closed` while the session still has an open turn in state `sent`. `reconcile_host` only looks at `active` sessions, so the first resume would inherit a 409. Plan B must release a `sent` open turn as `turn_not_delivered` when it ingests `session_parked` / `session_closed` (or when it resumes).
- **Key hub waiters on `conn_id`.** Waiters are keyed on `host_id` today, so an old connection's `unregister` fails waiters a newer connection of the same host now serves, and a timed-out request's `disconnect` kicks whichever connection is current. Filter both by `conn_id`. Until then this relies on every request timeout exceeding the collector's read deadline (asserted at compile time in `api.rs`).

Then, in order:
- **(2) Permission and elicitation.** The pending set and answer queue, including the teardown hooks this plan leaves out: cancel pending requests with `adapter_lost` / `session_parked` / `session_closed` / `host_restarted`, and drain the answer queue after reconciliation.
- **(3) Real auth and pairing.**
- **(4) Frontend shell.** Settle the generated TS optionals.
- **(5) Hats.** Re-resolution on resume, `hat_mismatch`, and close on re-assignment at `hello`.
- **(6) Gateway.** `SessionMcp::revoke` on park, close and exit.
- **(7) Distribution.** systemd `KillMode=mixed`.

---

_Generated with Claude AI — please review before distribution._

