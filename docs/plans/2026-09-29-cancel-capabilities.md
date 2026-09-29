# Cancel, capabilities and request hygiene (plan B2a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An operator can stop a running turn. The turn ends `cancelled` exactly once, even when the agent ignores the cancel or finishes first. Hosts announce what they can do, and the collector sends `park_session` only to hosts that can park. Two gaps plan B1 left open are closed: a host's rejection now reaches the store when the HTTP caller has already gone, and host shutdown never attaches an adapter it will not wait for.

**Architecture:**
- **Host.** The session actor learns `Cancel`. It sends ACP `session/cancel` for the turn in flight and maps the prompt's answer to `turn_ended{cancelled}`. If the adapter keeps running past a grace, the actor stops it. The connection dispatches `cancel_turn` and announces `hello.capabilities = [park]`. The session map gains a `closing` flag that shutdown sets under the same lock it takes the handles with.
- **Collector.** The hub keeps each connection's capabilities, and completes a request by the end of a turn as well as by a request id or a session. A waiter carries an `Undo`, and the socket task applies it when the host rejects the request, whether or not a handler is still waiting. `POST …/cancel` and the park gate sit on top.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, tokio-tungstenite 0.29, agent-client-protocol 2.2.0 (`CancelNotification`, `StopReason::Cancelled`), rusqlite 0.40, schemars/ts-rs codegen. No new dependencies. Nix flake dev shell.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md). The relevant sections are:
- §2.4 (`session/cancel` through the crate's typed sender);
- §3.3 (`cancel_turn`, the `park` capability gate, `hello.capabilities` as a closed list);
- §3.4 (the 60 s `cancel_turn` timeout; rejections correlated by `request_id`);
- §4.4 (`cancelled` after `cancel_turn`; exactly one `turn_ended` per started turn);
- §9 (`POST …/cancel`, `POST …/park` for hosts with the capability);
- §12 (scenario 4's refusals, applied to cancel).

It builds on the executed [resume plan](2026-09-28-resume.md) (plan B1). Read its "Execution status" and "After this plan" first. Its code wins over its task text, and every anchor below was taken from that code (`main` at `a57a1dd`).

## Scope

This is **plan B2a**. B1's "After this plan" scoped plan B2 as cancel, capabilities and all of model, axes and mode. With B1's two carry-overs added, that does not fit one plan of right-sized tasks. The config half alone needs:
- new fields on `session_started`, `start_session` and `resume_session`, whose literals and patterns sit at about 28 sites across the tests;
- `set_config` / `config_applied`;
- a migration with `session_catalog`;
- the host's switch order;
- a fake adapter catalogue;
- two routes.

So B2 is split. **B2a** (this plan) is cancel, capabilities and the carry-overs. **B2b** is model, axes and mode, scoped under "After this plan".

**In:**
- Frames: `cancel_turn`, `hello.capabilities` (`Capability`, a lenient `Capabilities` list). REST: `CancelResponse`.
- Fake adapter: `session/cancel` (stop and answer `cancelled`), `ignore_cancel`, `cancel_error`.
- Host: `SessionCmd::Cancel` → `session/cancel`; `StopReason::Cancelled` → `turn_ended{cancelled}`; the cancel grace; `not_running`.
- Host connection: `cancel_turn` dispatch; `hello.capabilities = [park]`; the `closing` flag.
- Collector: capabilities per connection; turn-completed waiters; undo of rejected starts, resumes and prompts in the socket task; `POST …/cancel`; the park gate.

**Out** (B2b, or later plans; see "After this plan"):
- model, axes and mode;
- `set_config` / `config_applied`;
- catalogue extracts and `session_catalog`;
- the `images` and `projects` capabilities (they arrive with image prompts and with the probes);
- cancelling pending permissions on `cancel_turn` (`turn_cancelled`): there are no pending requests yet;
- permission and elicitation; real auth; hats; gateway; frontend; distribution.

**Where B1's hand-offs land:**

| B1 "After this plan" / "Execution status" | Here |
|---|---|
| `cancel_turn` (§3.3, `POST …/cancel`); the collector's waiter matches the turn | Tasks 1–4, 6, 8, 9 |
| `hello.capabilities` (`projects`, `images`, `park`); refuse `park_session` without `park`; drops plan A decision 12 | Tasks 1, 6, 8 |
| Model, axes and mode; `set_config`; catalogue extracts; fake `session/set_config_option` | **B2b** |
| A dropped HTTP handler can leave a session `starting` (or a turn `sent`) while its host stays connected | Task 7 |
| A resume waiting behind a close can spawn after host shutdown took the sessions map | Task 5 |
| The frontend must offer close for a `starting` session whose host is offline | Frontend plan (unchanged) |

## Decisions this plan makes where the spec is silent

These are proposed by the plan author and were reviewed by an advisor model on 2026-09-29. The maintainer confirms them at plan review. The tasks implement them as written.

1. **`cancel_turn` is completed by its turn's `turn_ended`, whatever the outcome.** The collector's waiter matches the turn (B1's hand-off), not a fact carrying the request id. `POST …/cancel` answers 202 `CancelResponse {turn_id, outcome}`. A turn that finished just before the cancel reached the agent answers `completed`. The host writes that end to the outbox before its `not_running` rejection, and outboxed facts go on the wire before every reply (plan A, "Wire order"). A cancel request writes no collector event: §8's list has no `operator_cancelled`, and the turn's `turn_ended{cancelled}` is the timeline's record.
2. **The host takes the outcome from the prompt's answer.**
   - Stop reason `cancelled` → `turn_ended{cancelled}`, with or without a hennery cancel.
   - Any other stop reason → `completed`. The agent finished first.
   - An error after a cancel was sent → `cancelled`, with the error kept. An agent's aborted work may throw.
   - Without a cancel, an error is `failed`, as before.
3. **An adapter that ignores `session/cancel` is stopped after `CANCEL_GRACE` (20 s).** The turn ends `cancelled`, with an error saying so. The adapter's process group is killed, a `host_note{note: "cancel_unanswered"}` explains why, and `session_parked{operator}` detaches the session. 20 s plus the 5 s kill grace stays below the collector's 60 s cancel timeout. Without this bound, that timeout would drop the whole host connection (plan A decision 1). The adapter cannot take another prompt while the old one runs, so leaving it attached is not an option. The spec's park reasons are a closed list, so the reason is `operator` (the operator asked to stop the turn) and the note carries the specifics.
4. **Cancel refusals:**
   - 409 `not_attached`: the session is not `active` (presumed parked included), or its host is not ready.
   - 409 `no_open_turn`: no turn is open.
   - 409 `not_running`: the host has no such turn in flight.
   - 404: an unknown session.
   A cancel for a `sent` turn is sent: its prompt precedes it on the same socket, so the actor sees the turn first. A repeated cancel for a turn already being cancelled changes nothing on the host.
5. **Capabilities.**
   - `hello.capabilities` is a `Capabilities` newtype over `Vec<Capability>`. It deserializes leniently: an unknown entry from a newer host is skipped, never a reason to refuse the `hello`. An absent field means none.
   - The hennery host announces only `park`. `projects` and `images` are not implemented yet, and the collector must never be told otherwise.
   - The collector keeps capabilities per connection in the hub, not in the store. A host that reconnects on an older build loses them at once.
   - `POST …/park` to a host without `park` answers 409 `park_unsupported`, and nothing is sent. This reverses plan A's decision 12.
6. **A rejection is applied by the socket task, keyed by request id.** A start, resume or prompt registers an `Undo` with its waiter:
   - `Undo::Start` is `mark_failed_if_starting` with the host's code;
   - `Undo::Prompt` is `abandon_turn`.
   `ws.rs` applies the undo when the host's `error` arrives, then signals the waiter. The HTTP answer and the store therefore agree, and the store is right even when hyper has dropped the handler because its client left. The waiter entry outlives a dropped handler; only its answer, its connection's loss or its timeout removes it. Handlers keep only the never-sent (`host_offline`) path. A start's rejection now fails the session only while it is still `starting`, like a resume.
7. **Host shutdown closes the session map.** `shut_down` sets `closing` and takes the handles under one lock. `spawn_or_restart` checks `closing` under that lock and drops a start or resume that arrives afterwards, without an answer. The connection is already gone, and the collector reconciles the start after the next handshake (`start_not_delivered`).

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- `cargo fmt --all --check` (`max_width = 120`) and `cargo clippy --workspace --all-targets --locked -- -D warnings` pass after every task; `cargo test --workspace --locked` passes after every task.
- Generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated with `cargo run -p hennery-proto --bin gen` whenever a wire type changes and must pass `cargo run -p hennery-proto --bin gen -- --check`. In `codegen.rs`, new root types (frames, REST payloads) go in both `add!` lists and new nested types go in the `render_ts` list.
- ACP payloads are forwarded verbatim as `serde_json::Value` and never scrubbed; stderr tails and `host_note` text are scrubbed (ACP core §2.3).
- Every state-bearing fact from the host is a sequenced `session` frame through the outbox; only rejections (`error`) bypass it (ACP core §3.3).
- The collector acks a frame only after its transaction commits; ingest is idempotent on `(session_id, seq)`.
- "Every started turn ends with exactly one `turn_ended`, outcome one of: … `cancelled` — after `cancel_turn`" (ACP core §4.4).
- "`capabilities` is a closed list: `projects` (project enumeration and browsing), `images` (image content blocks in prompts), `park` (explicit park). The collector never sends a frame, or a prompt containing images, to a host that lacks the capability" (ACP core §3.3).
- "Every rejection, including a failed resume, is correlated by `request_id` alone and returns immediately" (ACP core §3.4).
- Collector timeouts: start and resume 90 s, prompt 60 s, `set_config`, `cancel_turn`, `park_session`, `close_session` 60 s, all ≥ the 45 s read deadline (ACP core §3.4).
- The collector sends no request to a host before that connection's post-`resend_complete` reconciliation (ACP core §5.1 step 4).
- No global installs: tooling comes from the flake dev shell.
- Commits: Conventional Commits (`feat(host): …`), made with the repository's own identity (gmail, unsigned); push the feature branch after every completed task; never push `main`.

## Review Focus

These are the five inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **An agent stuck in a long tool call ignores Stop.** Expected: within the grace the turn ends `cancelled` once, the adapter's whole group is killed, a `host_note` says why, and the session parks. The operator is never left with a 60 s timeout that drops every session's connection. (Task 3: `an_adapter_that_ignores_a_cancel_is_stopped_after_the_grace`)
2. **Stop clicked as the turn finishes, or clicked twice.** Expected: one end per turn. A cancel that lost the race answers how the turn really ended (`completed`), and a double click sends one `session/cancel`. An agent that answers an aborted prompt with an error still ends `cancelled`. (Task 3: `a_cancel_ends_the_turn_cancelled_and_the_session_stays_attached`, `a_cancel_for_a_turn_that_is_not_running_is_refused_not_running`, `a_cancelled_prompt_answered_with_an_error_still_ends_cancelled`; Task 8: `a_cancel_that_loses_the_race_with_the_turns_end_answers_how_it_ended`)
3. **A browser tab closed, or a request that times out client-side, while the host rejects the start, resume or prompt.** Expected: the store still records the rejection. The session is `failed` with the host's code, never stuck `starting`, and the turn slot is freed, never a permanent 409 `turn_in_progress` while the host stays connected. (Task 7: the three `…_rejected_after_its_caller_gave_up_…` tests)
4. **A newer host with a capability this collector does not know, or an older host with none.** Expected: the `hello` is accepted and the known capabilities are kept. Park to a host without `park` is refused 409 `park_unsupported`, and nothing reaches the host. (Task 1: `hello_capabilities_skip_unknown_entries_and_default_to_none`; Task 6: `a_hello_with_an_unknown_capability_is_accepted_with_the_known_ones`, `capabilities_belong_to_the_hosts_current_connection`; Task 8: `park_goes_only_to_a_host_that_announced_it_can_park`)
5. **`systemctl stop` (or Ctrl-C) on a host while a resume waits behind a close.** Expected: no adapter is launched after shutdown began, so none is SIGKILLed without its grace or orphaned. (Task 5: `a_resume_waiting_behind_a_close_never_attaches_after_host_shutdown`, with an adapter that ignores SIGTERM so the close takes its whole grace)

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/frames.rs`, `rest.rs`, `codegen.rs` | `Capability`, `Capabilities`, `hello.capabilities`, `CancelTurn`, `CancelResponse` | 1 |
| `crates/hennery-testkit/src/lib.rs`, `src/bin/hennery-fake-acp.rs` | Fake adapter: `session/cancel`, `ignore_cancel`, `cancel_error` | 2 |
| `crates/hennery-host/src/session.rs` | `SessionCmd::Cancel`, `CANCEL_GRACE`, outcome mapping, stopping an adapter that ignores a cancel | 3 |
| `crates/hennery-host/src/connection.rs` | `hello.capabilities` (1); `cancel_turn` dispatch (4); `SessionMap` with `closing` (5) | 1, 4, 5 |
| `crates/hennery-sessions/src/hub.rs`, `ws.rs` | Capabilities per connection, `CompletedBy`, `request_for_turn` / `resolve_turn` (6); `Undo`, `request_with_undo`, `undo_for`, `undo_rejected` (7) | 6, 7 |
| `crates/hennery-sessions/src/api.rs` | Handlers without rejection writes (7); `POST …/cancel`, the park gate (8) | 7, 8 |
| `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/{fake_acp,host_session,host_connection,reconcile,e2e,auth,ws_ingest_error}.rs`, `crates/hennery-sessions/tests/hub.rs` | Tests | all |

All commands run from the repository root inside the dev shell (`nix develop`, or direnv). Work on a feature branch off `main` (e.g. `feat/cancel-capabilities`). Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "Replace the whole of `path` with:" overwrites the file.
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file, then "with:" and its replacement.

The plan was built and tested by applying its own blocks in this way, task by task, to plan B1's merge commit (`a57a1dd`).

---

### Task 1: Wire types for `cancel_turn` and `hello.capabilities`

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`
- Modify: `crates/hennery-host/src/connection.rs` (the host announces `park`; `cancel_turn` refused until Task 4)
- Modify (compile only): `crates/hennery-testkit/tests/auth.rs`, `crates/hennery-testkit/tests/ws_ingest_error.rs`, `crates/hennery-testkit/tests/reconcile.rs`
- Test: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/host_connection.rs`
- Generated (committed): `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`

**Interfaces:**
- Produces: `hennery_proto::frames::Capability` (`projects` | `images` | `park`, snake_case) and `hennery_proto::frames::Capabilities(pub Vec<Capability>)` with `fn has(&self, Capability) -> bool` and `Default`. It deserializes leniently: unknown entries are skipped.
- Produces: `HostFrame::Hello` gains `capabilities: Capabilities` (`#[serde(default)]`, between `token` and `attached_sessions`).
- Produces: `CollectorFrame::CancelTurn { request_id, session_id, turn_id }` (tag `cancel_turn`).
- Produces: `hennery_proto::rest::CancelResponse { turn_id: String, outcome: TurnOutcome }`.
- The host announces `Capabilities(vec![Capability::Park])` from here on. Until Task 4 it refuses `cancel_turn` with `error{code: "unsupported"}`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        CollectorFrame::CloseSession {
            request_id: "r".into(),
            session_id: "s".into(),
        },
    ];
```

with:

```rust
        CollectorFrame::CloseSession {
            request_id: "r".into(),
            session_id: "s".into(),
        },
        CollectorFrame::CancelTurn {
            request_id: "r".into(),
            session_id: "s".into(),
            turn_id: "t".into(),
        },
    ];
```

Append to `crates/hennery-proto/tests/frames.rs`:

```rust
#[test]
fn cancel_turn_and_its_answer_use_the_spec_field_names() {
    let cancel = CollectorFrame::CancelTurn {
        request_id: "r".into(),
        session_id: "s".into(),
        turn_id: "t".into(),
    };
    assert_eq!(
        serde_json::to_value(&cancel).unwrap(),
        json!({"type": "cancel_turn", "request_id": "r", "session_id": "s", "turn_id": "t"})
    );
    let answer = hennery_proto::rest::CancelResponse {
        turn_id: "t".into(),
        outcome: TurnOutcome::Cancelled,
    };
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({"turn_id": "t", "outcome": "cancelled"})
    );
}

#[test]
fn hello_capabilities_skip_unknown_entries_and_default_to_none() {
    use hennery_proto::frames::{Capabilities, Capability};
    let hello = |extra: serde_json::Value| {
        let mut v = json!({
            "type": "hello", "protocol_version": "1.0", "host_version": "0", "host_id": "h",
            "token": "t", "attached_sessions": []
        });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        match serde_json::from_value::<HostFrame>(v).unwrap() {
            HostFrame::Hello { capabilities, .. } => capabilities,
            other => panic!("expected hello, got {other:?}"),
        }
    };
    let newer = hello(json!({"capabilities": ["park", "teleport", "images"]}));
    assert_eq!(newer, Capabilities(vec![Capability::Park, Capability::Images]));
    assert!(newer.has(Capability::Park) && !newer.has(Capability::Projects));
    assert_eq!(hello(json!({})), Capabilities::default());
    let sent = HostFrame::Hello {
        protocol_version: "1.0".into(),
        host_version: "0".into(),
        host_id: "h".into(),
        token: "t".into(),
        capabilities: Capabilities(vec![Capability::Park]),
        attached_sessions: vec![],
    };
    assert_eq!(serde_json::to_value(&sent).unwrap()["capabilities"], json!(["park"]));
}
```

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
// Plan B2a: cancel and capabilities over the connection.

#[tokio::test]
async fn a_host_announces_that_it_can_park() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "capabilities", slow_fake())));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let (_sink, mut stream) = tokio_tungstenite::accept_async(tcp).await.unwrap().split();
    let HostFrame::Hello { capabilities, .. } = read_host_frame(&mut stream).await else {
        panic!("expected hello");
    };
    assert_eq!(
        capabilities,
        hennery_proto::frames::Capabilities(vec![hennery_proto::frames::Capability::Park])
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-proto --test frames`
Expected: compile error: no variant `CancelTurn`, no field `capabilities` on `Hello`, unresolved `CancelResponse`, `Capabilities`, `Capability`.

- [ ] **Step 3: Add the types**

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
/// How a turn ended. Exactly one `turn_ended` per accepted turn (ACP core §4.4).
```

with:

```rust
/// A feature a host implements, announced in `hello` (ACP core §3.3). The
/// collector never sends a frame that needs a capability to a host that
/// lacks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Project enumeration and browsing.
    Projects,
    /// Image content blocks in prompts.
    Images,
    /// Explicit park (`park_session`).
    Park,
}

/// `hello.capabilities`. Deserialized leniently: a capability this build
/// does not know (a newer host, a minor protocol bump) is skipped, never a
/// reason to refuse the whole `hello`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema, TS)]
pub struct Capabilities(pub Vec<Capability>);

impl Capabilities {
    pub fn has(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }
}

impl<'de> Deserialize<'de> for Capabilities {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw: Vec<Value> = Deserialize::deserialize(deserializer)?;
        Ok(Self(
            raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect(),
        ))
    }
}

/// How a turn ended. Exactly one `turn_ended` per accepted turn (ACP core §4.4).
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        token: String,
        attached_sessions: Vec<AttachedSession>,
    },
```

with:

```rust
        token: String,
        /// What this host implements (a closed list, ACP core §3.3). Absent
        /// means none.
        #[serde(default)]
        capabilities: Capabilities,
        attached_sessions: Vec<AttachedSession>,
    },
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    Ack {
        session_id: String,
        #[ts(type = "number")]
        ack_seq: u64,
    },
```

with:

```rust
    /// Stop the turn in flight: `session/cancel` to the adapter. Completed
    /// by that turn's `turn_ended` (ACP core §3.3, §4.4), whatever its
    /// outcome: a turn that finished first is not cancelled.
    CancelTurn {
        request_id: String,
        session_id: String,
        turn_id: String,
    },
    Ack {
        session_id: String,
        #[ts(type = "number")]
        ack_seq: u64,
    },
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// `POST /api/sessions/{id}/cancel`: how the open turn ended. `cancelled`,
/// unless it finished (or failed) before the cancel reached the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct CancelResponse {
    pub turn_id: String,
    pub outcome: crate::frames::TurnOutcome,
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::LifecycleResponse,
        rest::SessionDetail,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::LifecycleResponse,
        rest::SessionDetail,
        rest::CancelResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
    add!(
        frames::AttachedSession,
```

with:

```rust
    add!(
        frames::Capability,
        frames::Capabilities,
        frames::AttachedSession,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::OpenTurn,
        rest::SessionDetail,
    );
    out
```

with:

```rust
        rest::OpenTurn,
        rest::SessionDetail,
        rest::CancelResponse,
    );
    out
```

- [ ] **Step 4: The host announces `park`; keep the workspace compiling**

The new variant breaks the host's frame match, and the new field breaks four `Hello` literals. The `cancel_turn` arm is correct until Task 4 replaces it.

In `crates/hennery-host/src/connection.rs`, replace:

```rust
use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame};
```

with:

```rust
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame};
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            token: cfg.token.clone(),
            attached_sessions: attached,
```

with:

```rust
            token: cfg.token.clone(),
            // Every hennery host can park. `projects` and `images` come
            // with the probes and with image prompts.
            capabilities: Capabilities(vec![Capability::Park]),
            attached_sessions: attached,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

with:

```rust
        // Wired to the session actor with cancel; until then it is refused,
        // so the collector's waiter returns at once.
        CollectorFrame::CancelTurn { request_id, .. } => uplink.reply(HostFrame::Error {
            request_id,
            code: "unsupported".into(),
            message: "this host cannot cancel turns yet".into(),
        }),
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
            token: "not-the-token".into(),
            attached_sessions: vec![],
```

with:

```rust
            token: "not-the-token".into(),
            capabilities: Default::default(),
            attached_sessions: vec![],
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
            token: TOKEN.into(),
            attached_sessions: vec![],
```

with:

```rust
            token: TOKEN.into(),
            capabilities: Default::default(),
            attached_sessions: vec![],
```

The scripted host in `reconcile.rs` stands in for a real one, so it announces `park` too.

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use hennery_proto::frames::{AttachedSession, CollectorFrame, HostFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            token: TOKEN.into(),
            attached_sessions: attached,
```

with:

```rust
            token: TOKEN.into(),
            capabilities: Capabilities(vec![Capability::Park]),
            attached_sessions: attached,
```

- [ ] **Step 5: Regenerate, then run the tests**

Run: `cargo run -p hennery-proto --bin gen && cargo test --workspace && cargo run -p hennery-proto --bin gen -- --check`
Expected: `wrote schema/…`, `wrote web/…`. All tests pass, including `cancel_turn_and_its_answer_use_the_spec_field_names`, `hello_capabilities_skip_unknown_entries_and_default_to_none`, `a_host_announces_that_it_can_park` and the two `generated_*_matches_the_checked_in_copy` tests. `--check` exits 0. The generated TypeScript has `export type Capabilities = Array<Capability>;`.

- [ ] **Step 6: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
git add crates/hennery-proto crates/hennery-host/src/connection.rs crates/hennery-testkit/tests schema web
git commit -m "feat(proto): add cancel_turn and hello capabilities"
```

---

### Task 2: Fake adapter: `session/cancel`

**Files:**
- Modify: `crates/hennery-testkit/src/lib.rs`, `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`
- Test: `crates/hennery-testkit/tests/fake_acp.rs`

**Interfaces:**
- Produces: the fake handles the `session/cancel` notification. The prompt in flight stops streaming (it checks between chunks, while it waits `chunk_delay_ms`) and answers stop reason `cancelled`. A prompt that is not running is unaffected, because a new prompt clears any earlier cancel.
- Produces: new `FakeScript` fields, both `#[serde(default)]`:
  - `ignore_cancel: bool`: receive the notification and keep streaming, as an adapter that does not honour cancellation would;
  - `cancel_error: Option<i32>`: answer a cancelled prompt with this JSON-RPC error code instead of `cancelled`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/fake_acp.rs`:

```rust
/// A prompt followed at once by `session/cancel`.
fn cancelled_prompt_requests() -> Vec<Value> {
    let mut requests = session_requests();
    requests.push(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"fake-session-1"}}));
    requests
}

#[test]
fn session_cancel_stops_the_prompt_and_answers_cancelled() {
    let script = r#"{"chunks":["a","b","c","d","e"],"chunk_delay_ms":300}"#;
    let out = exchange_until(script, &cancelled_prompt_requests(), 3);
    let chunks = out.iter().filter(|m| m["method"] == "session/update").count();
    assert!(chunks < 5, "the prompt ran to its end: {out:?}");
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "cancelled", "{out:?}");
    let script = r#"{"chunks":["a","b","c","d","e"],"chunk_delay_ms":300,"cancel_error":-32603}"#;
    let out = exchange_until(script, &cancelled_prompt_requests(), 3);
    assert_eq!(out.last().unwrap()["error"]["code"], -32603, "{out:?}");
}

#[test]
fn ignore_cancel_runs_the_prompt_to_its_end() {
    let script = r#"{"chunks":["a","b","c"],"chunk_delay_ms":50,"ignore_cancel":true}"#;
    let out = exchange_until(script, &cancelled_prompt_requests(), 3);
    let chunks = out.iter().filter(|m| m["method"] == "session/update").count();
    assert_eq!(chunks, 3, "{out:?}");
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "end_turn", "{out:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test fake_acp`
Expected: `session_cancel_stops_the_prompt_and_answers_cancelled` FAILS: the fake ignores the notification, streams all five chunks and answers `end_turn`. (`ignore_cancel_runs_the_prompt_to_its_end` passes already: an unknown script field is ignored.)

- [ ] **Step 3: Implement**

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    /// Advertise `loadSession: false` in `initialize`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_load_session: bool,
}
```

with:

```rust
    /// Advertise `loadSession: false` in `initialize`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_load_session: bool,
    /// Receive `session/cancel` but keep streaming the prompt as if it
    /// never came (an adapter that does not honour cancellation).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_cancel: bool,
    /// Answer a cancelled prompt with this JSON-RPC error code instead of
    /// the `cancelled` stop reason (an agent whose aborted work throws).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_error: Option<i32>,
}
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            no_load_session: false,
        }
```

with:

```rust
            no_load_session: false,
            ignore_cancel: false,
            cancel_error: None,
        }
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionNotification,
    SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeScript, SCRIPT_ENV};
use std::time::Duration;
```

with:

```rust
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse,
    SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeScript, SCRIPT_ENV};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
    let load_session = !script.no_load_session;
    Agent
```

with:

```rust
    let load_session = !script.no_load_session;
    // `session/cancel` for the prompt in flight: set by the notification,
    // cleared when a prompt starts. Handlers run in arrival order, so a
    // cancel sent right after its prompt is never cleared by that prompt.
    let cancel = Arc::new(watch::channel(false).0);
    Agent
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
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
```

with:

```rust
        .on_receive_notification(
            {
                let cancel = cancel.clone();
                let ignore = script.ignore_cancel;
                async move |_n: CancelNotification, _cx| {
                    if !ignore {
                        cancel.send_replace(true);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                async move |req: PromptRequest, responder, cx| {
                    let script = script.clone();
                    let cx2 = cx.clone();
                    cancel.send_replace(false);
                    let mut cancelled = cancel.subscribe();
                    cx.spawn(async move {
                        for (sent, chunk) in script.chunks.into_iter().enumerate() {
                            if script.exit_after_chunks == Some(sent) {
                                crash().await;
                            }
                            // A cancelled prompt stops streaming and answers
                            // `cancelled`, as ACP asks of an agent.
                            tokio::select! {
                                _ = tokio::time::sleep(Duration::from_millis(script.chunk_delay_ms)) => {}
                                _ = cancelled.wait_for(|c| *c) => {
                                    return match script.cancel_error {
                                        Some(code) => responder
                                            .respond_with_error(agent_client_protocol::Error::new(code, "aborted")),
                                        None => responder.respond(PromptResponse::new(StopReason::Cancelled)),
                                    };
                                }
                            }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test fake_acp`
Expected: all 9 pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-testkit
git commit -m "test(testkit): fake adapter honours session/cancel"
```

---

### Task 3: Session actor: cancel the turn in flight

**Files:**
- Modify: `crates/hennery-host/src/session.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Consumes: the fake's `session/cancel`, `ignore_cancel` and `cancel_error` (Task 2).
- Produces: `SessionCmd::Cancel { request_id: String, turn_id: String }`.
  - For the turn in flight, the first cancel sends ACP `session/cancel` (`CancelNotification`) and starts that turn's grace; a repeated cancel changes nothing.
  - For any other turn, or with no turn in flight, the cancel is answered `error{code: "not_running"}`.
  - In the post-drive drain it is answered `not_attached`, like every other command.
- Produces: `pub const CANCEL_GRACE: Duration` (20 s) and `SessionOptions::cancel_grace: Duration` (defaults to it).
- Produces: turn outcomes (decision 2):
  - stop reason `cancelled` → `TurnOutcome::Cancelled` (with `stop_reason: Some("cancelled")`);
  - an error after a cancel was sent → `Cancelled`, with the error;
  - otherwise as before.
- Produces: past the grace, the actor emits `turn_ended{cancelled, error}`, kills the adapter's group, then emits `host_note{note: "cancel_unanswered"}` and `session_parked{operator}`, and ends (decision 3).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
// Plan B2a: cancel (ACP core §3.3 `cancel_turn`, §4.4).

fn cancel(request_id: &str, turn_id: &str) -> SessionCmd {
    SessionCmd::Cancel {
        request_id: request_id.into(),
        turn_id: turn_id.into(),
    }
}

/// Every `turn_ended` in the outbox: (turn id, outcome, error).
fn turn_ends(frames: &[HostFrame]) -> Vec<(String, TurnOutcome, Option<String>)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::TurnEnded {
                        turn_id,
                        outcome,
                        error,
                        ..
                    },
                ..
            } => Some((turn_id.clone(), *outcome, error.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_cancel_ends_the_turn_cancelled_and_the_session_stays_attached() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&slow_script()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    // Twice, as a retried request would: one `session/cancel`, one end.
    assert!(handle.send(cancel("rc1", "t1")));
    assert!(handle.send(cancel("rc2", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(turn_ends(&frames), [("t1".to_string(), TurnOutcome::Cancelled, None)]);
    let stop_reasons: Vec<Option<String>> = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::TurnEnded { stop_reason, .. },
                ..
            } => Some(stop_reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(stop_reasons, [Some("cancelled".to_string())]);
    assert!(
        kinds(&frames).iter().filter(|k| k.starts_with("update:")).count() < 20,
        "the turn ran to its end: {:?}",
        kinds(&frames)
    );
    assert_eq!(handle.open_turn_id(), None);
    // Still attached: the next prompt runs.
    assert!(handle.send(SessionCmd::Prompt {
        request_id: "r2".into(),
        turn_id: "t2".into(),
        content: vec![json!({"type":"text","text":"again"})],
    }));
    wait_until(&uplink, |f| turn_ends(f).len() == 2).await;
    assert!(replies.try_recv().is_err(), "a cancel was answered with an error");
}

#[tokio::test]
async fn a_cancel_for_a_turn_that_is_not_running_is_refused_not_running() {
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(&FakeScript::default()),
        std::env::temp_dir(),
    );
    wait_until(&uplink, has("session_started")).await;
    // No turn at all, then a turn that has already ended.
    assert!(handle.send(cancel("rc1", "t0")));
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
    assert!(handle.send(cancel("rc2", "t1")));
    let mut refused = Vec::new();
    for _ in 0..2 {
        match tokio::time::timeout(Duration::from_secs(5), replies.recv())
            .await
            .unwrap()
            .unwrap()
        {
            HostFrame::Error { request_id, code, .. } => refused.push((request_id, code)),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        refused,
        [
            ("rc1".to_string(), "not_running".to_string()),
            ("rc2".to_string(), "not_running".to_string())
        ]
    );
    let frames = uplink.pending().unwrap();
    assert_eq!(turn_ends(&frames), [("t1".to_string(), TurnOutcome::Completed, None)]);
}

#[tokio::test]
async fn an_adapter_that_ignores_a_cancel_is_stopped_after_the_grace() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        ignore_cancel: true,
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
            cancel_grace: Duration::from_millis(300),
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    let grandchild = read_pid(&pid_file).await;
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert!(handle.send(cancel("rc", "t1")));
    let frames = wait_until(&uplink, has("session_parked:operator")).await;
    let ends = turn_ends(&frames);
    assert_eq!(ends.len(), 1, "{:?}", kinds(&frames));
    assert_eq!((ends[0].0.as_str(), ends[0].1), ("t1", TurnOutcome::Cancelled));
    let tail: Vec<String> = kinds(&frames)
        .into_iter()
        .filter(|k| !k.starts_with("update:"))
        .collect();
    assert_eq!(
        tail,
        [
            "session_started",
            "turn_started",
            "turn_ended",
            "host_note:cancel_unanswered",
            "session_parked:operator"
        ]
    );
    wait_dead(grandchild).await;
    wait_ended(&handle).await;
}

/// An agent may answer an aborted prompt with an error rather than the
/// `cancelled` stop reason: after a cancel that is still `cancelled`, with
/// the error kept; without one it would be `failed`.
#[tokio::test]
async fn a_cancelled_prompt_answered_with_an_error_still_ends_cancelled() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        cancel_error: Some(-32603),
        ..slow_script()
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
    wait_until(&uplink, has("turn_started")).await;
    assert!(handle.send(cancel("rc", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let ends = turn_ends(&frames);
    assert_eq!(ends.len(), 1);
    assert_eq!((ends[0].0.as_str(), ends[0].1), ("t1", TurnOutcome::Cancelled));
    assert!(ends[0].2.as_deref().is_some_and(|e| e.contains("aborted")), "{ends:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session cancel`
Expected: compile error: no variant `SessionCmd::Cancel`, no field `cancel_grace` on `SessionOptions`.

- [ ] **Step 3: Implement**

In `crates/hennery-host/src/session.rs`, replace:

```rust
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionId,
};
```

with:

```rust
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    PromptResponse, SessionId, StopReason,
};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// Default idle window before the reaper parks a session (ACP core §4.7).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
```

with:

```rust
/// Default idle window before the reaper parks a session (ACP core §4.7).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long an adapter gets to end a turn after `session/cancel` before the
/// host stops it. Well below the collector's 60 s cancel timeout (ACP core
/// §3.4), so the collector hears the turn end, not a timeout that would
/// drop the whole host connection.
pub const CANCEL_GRACE: Duration = Duration::from_secs(20);
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// Operator close: end any turn, kill the group, `session_closed`.
    Close { request_id: String },
}
```

with:

```rust
    /// Operator close: end any turn, kill the group, `session_closed`.
    Close { request_id: String },
    /// Operator cancel of the turn in flight: `session/cancel` to the
    /// adapter; the turn's `turn_ended` completes it (ACP core §4.4).
    Cancel { request_id: String, turn_id: String },
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
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

with:

```rust
    /// (`session_parked{idle}`); `None` disables the reaper.
    pub idle_timeout: Option<Duration>,
    /// How long a cancelled turn may run on before the adapter is stopped.
    pub cancel_grace: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            start_timeout: START_TIMEOUT,
            kill_grace: KILL_GRACE,
            idle_timeout: Some(IDLE_TIMEOUT),
            cancel_grace: CANCEL_GRACE,
        }
    }
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
struct Turn {
    id: String,
    reply: Reply,
}
```

with:

```rust
struct Turn {
    id: String,
    reply: Reply,
    /// Set once `session/cancel` went out: the adapter must end the turn by
    /// then, or it is stopped.
    cancel_deadline: Option<Instant>,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Restart { request_id }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id } => {
```

with:

```rust
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Restart { request_id }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id }
                | SessionCmd::Cancel { request_id, .. } => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let mut idle_since = Instant::now();
        loop {
            tokio::select! {
```

with:

```rust
        let mut idle_since = Instant::now();
        loop {
            let cancel_at = turn.as_ref().and_then(|t| t.cancel_deadline);
            tokio::select! {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                        let reply = conn.send_request(PromptRequest::new(agent_session.clone(), blocks)).block_task();
                        turn = Some(Turn { id: turn_id, reply: Box::pin(reply) });
                    }
```

with:

```rust
                        let reply = conn.send_request(PromptRequest::new(agent_session.clone(), blocks)).block_task();
                        turn = Some(Turn { id: turn_id, reply: Box::pin(reply), cancel_deadline: None });
                    }
                    Some(SessionCmd::Cancel { request_id, turn_id }) => match turn.as_mut() {
                        Some(running) if running.id == turn_id => {
                            // A repeated cancel changes nothing: the one
                            // already sent is still being honoured.
                            if running.cancel_deadline.is_none() {
                                if let Err(err) = conn.send_notification(CancelNotification::new(agent_session.clone())) {
                                    tracing::warn!(session_id = %self.session_id, error = %err, "session/cancel not sent");
                                }
                                running.cancel_deadline = Some(Instant::now() + self.options.cancel_grace);
                            }
                        }
                        // Not started here, or already ended: its end (if
                        // any) is in the outbox ahead of this answer.
                        _ => self.reject(request_id, "not_running", "that turn is not running".into()),
                    },
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    let ended = turn.take().expect("a reply implies a turn");
                    idle_since = Instant::now();
```

with:

```rust
                    let ended = turn.take().expect("a reply implies a turn");
                    let cancelling = ended.cancel_deadline.is_some();
                    idle_since = Instant::now();
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    match result {
                        Ok(response) => self.end_turn(ended.id, TurnOutcome::Completed, stop_reason(&response), None),
                        Err(err) => {
                            let exited = adapter.exited_within(EXIT_SETTLE).await;
                            // Updates that arrived during the wait above are
                            // also ahead of this turn's end.
                            self.drain_updates(&mut updates, Some(&ended.id));
                            if let Some(info) = exited {
                                return self.adapter_exited(info, &mut adapter, &mut updates, Some(ended)).await;
                            }
                            self.end_turn(ended.id, TurnOutcome::Failed, None, Some(err.to_string()));
                        }
                    }
                }
```

with:

```rust
                    match result {
                        // The agent reports a cancelled turn by its stop
                        // reason (ACP): anything else finished first.
                        Ok(response) => {
                            let outcome = if response.stop_reason == StopReason::Cancelled {
                                TurnOutcome::Cancelled
                            } else {
                                TurnOutcome::Completed
                            };
                            self.end_turn(ended.id, outcome, stop_reason(&response), None);
                        }
                        Err(err) => {
                            let exited = adapter.exited_within(EXIT_SETTLE).await;
                            // Updates that arrived during the wait above are
                            // also ahead of this turn's end.
                            self.drain_updates(&mut updates, Some(&ended.id));
                            if let Some(info) = exited {
                                return self.adapter_exited(info, &mut adapter, &mut updates, Some(ended)).await;
                            }
                            // An agent may answer an aborted prompt with an
                            // error instead of `cancelled`.
                            let outcome = if cancelling { TurnOutcome::Cancelled } else { TurnOutcome::Failed };
                            self.end_turn(ended.id, outcome, None, Some(err.to_string()));
                        }
                    }
                }
                _ = cancel_deadline(cancel_at) => {
                    let unanswered = turn.take().expect("a deadline implies a turn");
                    return self.stop_after_unanswered_cancel(&mut adapter, &mut updates, unanswered).await;
                }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The exit watcher's steps (ACP core §2.3): outstanding calls fail (the
```

with:

```rust
    /// The adapter kept running a turn past `cancel_grace` after
    /// `session/cancel`. It cannot take another prompt while that one runs
    /// (one turn at a time, ACP core §4.4), so it is stopped: the turn ends
    /// `cancelled` (what the operator asked for), a `host_note` says why,
    /// and the session parks.
    async fn stop_after_unanswered_cancel(
        &self,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Value>,
        turn: Turn,
    ) {
        let grace = self.options.cancel_grace;
        tracing::warn!(session_id = %self.session_id, ?grace, "adapter ignored session/cancel; stopping it");
        self.drain_updates(updates, Some(&turn.id));
        let message = format!("the adapter did not stop within {grace:?} of session/cancel");
        self.end_turn(turn.id, TurnOutcome::Cancelled, None, Some(message.clone()));
        adapter.terminate(self.options.kill_grace).await;
        self.emit(SessionBody::HostNote {
            note: "cancel_unanswered".into(),
            text: scrub(&format!("{message}; it was stopped")),
        });
        self.emit(SessionBody::SessionParked {
            reason: ParkReason::Operator,
        });
    }

    /// The exit watcher's steps (ACP core §2.3): outstanding calls fail (the
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// The in-flight prompt's reply, or never if no turn is running.
```

with:

```rust
/// Resolves when a cancelled turn's grace is up; never if no cancel is out.
async fn cancel_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The in-flight prompt's reply, or never if no turn is running.
```

The deadline is read into `cancel_at` before the `select!`, because `next_reply` already borrows `turn` mutably inside it.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_session`
Expected: all 30 pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host/src/session.rs crates/hennery-testkit/tests/host_session.rs
git commit -m "feat(host): cancel the turn in flight with session/cancel"
```

---

### Task 4: Host connection: dispatch `cancel_turn`

**Files:**
- Modify: `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Consumes: `SessionCmd::Cancel` (Task 3).
- Produces: `cancel_turn` goes to the session's live actor. With no live actor, it is answered `error{code: "not_attached"}` at once, like a prompt. This replaces Task 1's `unsupported` arm.

- [ ] **Step 1: Write the failing test**

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
#[tokio::test]
async fn cancel_turn_reaches_the_actor_and_a_cancel_for_a_detached_session_is_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "cancel", slow_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
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
    let cancel = |request_id: &str, session_id: &str| CollectorFrame::CancelTurn {
        request_id: request_id.into(),
        session_id: session_id.into(),
        turn_id: "t1".into(),
    };
    send_frame(&mut sink, &cancel("r3", "s1")).await;
    let ended = read_until(&mut stream, body_is("s1", "turn_ended")).await;
    let HostFrame::Session {
        body: hennery_proto::frames::SessionBody::TurnEnded { turn_id, outcome, .. },
        ..
    } = ended
    else {
        panic!("expected turn_ended, got {ended:?}");
    };
    assert_eq!(
        (turn_id.as_str(), outcome),
        ("t1", hennery_proto::frames::TurnOutcome::Cancelled)
    );
    send_frame(&mut sink, &cancel("r4", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p hennery-testkit --test host_connection cancel_turn_reaches`
Expected: FAIL. The host answers `r3` with `error{unsupported}`, and the 20-chunk turn ends `completed` (the assertion on the outcome fails).

- [ ] **Step 3: Implement**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        // Wired to the session actor with cancel; until then it is refused,
        // so the collector's waiter returns at once.
        CollectorFrame::CancelTurn { request_id, .. } => uplink.reply(HostFrame::Error {
            request_id,
            code: "unsupported".into(),
            message: "this host cannot cancel turns yet".into(),
        }),
```

with:

```rust
        CollectorFrame::CancelTurn {
            request_id,
            session_id,
            turn_id,
        } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::Cancel {
                    request_id: request_id.clone(),
                    turn_id,
                }) => {}
            _ => not_attached(uplink, request_id),
        },
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_connection`
Expected: all 14 pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host/src/connection.rs crates/hennery-testkit/tests/host_connection.rs
git commit -m "feat(host): dispatch cancel_turn to the session actor"
```

---

### Task 5: Host shutdown never attaches an adapter it will not wait for

**Files:**
- Modify: `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Produces: `Sessions` is `Arc<Mutex<SessionMap>>` (private), where `SessionMap { handles: HashMap<String, SessionHandle>, closing: bool }`.
  - `shut_down` sets `closing` and takes `handles` under one lock.
  - `spawn_or_restart` returns without spawning while `closing` is set (decision 7).
  - Nothing outside `connection.rs` changes.

The carry-over (B1 "Execution status"): `attach` spawns a task that waits for an ending actor's `finished()` and then calls `spawn_or_restart`. If host shutdown takes the map in between, the fresh actor is inserted into a map nobody reads. Shutdown never waits for it, and the runtime's drop SIGKILLs its adapter without the SIGTERM grace.

- [ ] **Step 1: Write the failing test**

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
/// A resume waiting behind a close must not attach once host shutdown has
/// taken the session map: shutdown would never wait for that adapter, and
/// the runtime would SIGKILL it without its grace. The adapter ignores
/// SIGTERM, so the close takes the whole 5 s kill grace, and shutdown
/// begins while the resume still waits.
#[tokio::test]
async fn a_resume_waiting_behind_a_close_never_attaches_after_host_shutdown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let spawns = dir.path().join("spawns");
    let stubborn = counting_fake_with(&spawns, "trap '' TERM;");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let host = tokio::spawn(hennery_host::run_until(
        host_with_fake(addr, "resume-after-shutdown", stubborn),
        async {
            let _ = stopped.await;
        },
    ));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    send_frame(
        &mut sink,
        &CollectorFrame::CloseSession {
            request_id: "r2".into(),
            session_id: "s1".into(),
        },
    )
    .await;
    send_frame(&mut sink, &resume("r3", "s1", 0, "fake-session-1")).await;
    // Nothing on the wire says the resume is queued; the close has most of
    // its 5 s grace left.
    tokio::time::sleep(Duration::from_secs(1)).await;
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), host)
        .await
        .expect("the host shut down")
        .unwrap()
        .unwrap();
    // Time for a wrongly attached adapter to launch.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(std::fs::read_to_string(&spawns).unwrap().lines().count(), 1);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p hennery-testkit --test host_connection never_attaches_after_host_shutdown`
Expected: FAIL after about 7 s with `left: 2, right: 1`. The resume spawned a second adapter once the closing actor finished, after shutdown had taken the map.

- [ ] **Step 3: Implement**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
type Sessions = Arc<Mutex<HashMap<String, SessionHandle>>>;
```

with:

```rust
/// The host's session actors, by session id.
#[derive(Default)]
struct SessionMap {
    handles: HashMap<String, SessionHandle>,
    /// Set by host shutdown when it takes `handles`: from then on no actor
    /// is spawned. A start or resume that was waiting behind an ending actor
    /// would otherwise attach an adapter that shutdown never sees, and that
    /// the runtime then SIGKILLs without its grace.
    closing: bool,
}

type Sessions = Arc<Mutex<SessionMap>>;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
```

with:

```rust
    let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let actors: Vec<_> = std::mem::take(&mut *sessions.lock().expect("sessions lock"))
        .into_values()
        .map(|handle| handle.finished())
        .collect();
```

with:

```rust
    let handles = {
        let mut map = sessions.lock().expect("sessions lock");
        map.closing = true;
        std::mem::take(&mut map.handles)
    };
    let actors: Vec<_> = handles.into_values().map(|handle| handle.finished()).collect();
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        let mut map = sessions.lock().expect("sessions lock");
        map.retain(|_, handle| !handle.is_ended());
        map.iter().map(|(id, h)| (id.clone(), h.clone())).collect()
```

with:

```rust
        let mut map = sessions.lock().expect("sessions lock");
        map.handles.retain(|_, handle| !handle.is_ended());
        map.handles.iter().map(|(id, h)| (id.clone(), h.clone())).collect()
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        .expect("sessions lock")
        .get(session_id)
```

with:

```rust
        .expect("sessions lock")
        .handles
        .get(session_id)
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let mut map = sessions.lock().expect("sessions lock");
    if let Some(handle) = map.get(&req.session_id).filter(|h| !h.is_ended() && !h.is_ending())
```

with:

```rust
    let mut map = sessions.lock().expect("sessions lock");
    if map.closing {
        // Host shutdown: the collector sees this connection end and
        // reconciles the start after the next handshake (ACP core §5.1).
        tracing::info!(session_id = %req.session_id, "host shutting down; not attaching");
        return;
    }
    if let Some(handle) = map
        .handles
        .get(&req.session_id)
        .filter(|h| !h.is_ended() && !h.is_ending())
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    map.insert(req.session_id, handle);
```

with:

```rust
    map.handles.insert(req.session_id, handle);
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_connection`
Expected: all 15 pass. Also check that the test is a real guard: turn `if map.closing {` into `if map.closing && false {`, watch `a_resume_waiting_behind_a_close_never_attaches_after_host_shutdown` fail with `left: 2`, then restore it.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-host/src/connection.rs crates/hennery-testkit/tests/host_connection.rs
git commit -m "fix(host): attach nothing once host shutdown has begun"
```

---

### Task 6: Hub: capabilities per connection, requests completed by a turn's end

**Files:**
- Modify: `crates/hennery-sessions/src/hub.rs`, `crates/hennery-sessions/src/ws.rs`
- Modify (compile only): `crates/hennery-testkit/tests/e2e.rs`
- Test: `crates/hennery-sessions/tests/hub.rs`, `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: `Capabilities`, `Capability`, `CollectorFrame::CancelTurn` (Task 1).
- Produces: `Hub::register(&self, host_id: &str, tx: mpsc::UnboundedSender<CollectorFrame>, capabilities: Capabilities) -> Option<Registration>`. `ws.rs` passes the `hello`'s capabilities.
- Produces: `Hub::has_capability(&self, host_id: &str, capability: Capability) -> bool`, which reads the host's current connection. A host that is not connected has none.
- Produces: `Hub::request_for_turn(&self, host_id, request_id, turn_id: &str, frame, timeout) -> Result<SessionBody, RequestError>` and `Hub::resolve_turn(&self, turn_id: &str, fact: SessionBody)`. `ws.rs` calls `resolve_turn` on every ingested `turn_ended`. Rejections still match `request_id`.
- Internal: `Waiter.session_id: Option<String>` becomes `completed_by: CompletedBy { Request | Session(String) | Turn(String) }`. `resolve_session` and `resolve_turn` share `resolve_where`. `resolve(request_id)` is unchanged.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
use hennery_proto::frames::{CollectorFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, ParkReason, SessionBody, TurnOutcome};
```

In `crates/hennery-sessions/tests/hub.rs`, replace:

```rust
    let registration = hub.register("h", tx).expect("no live connection for h");
```

with:

```rust
    let registration = hub
        .register("h", tx, Capabilities::default())
        .expect("no live connection for h");
```

Append to `crates/hennery-sessions/tests/hub.rs`:

```rust
fn cancel(request_id: &str) -> CollectorFrame {
    CollectorFrame::CancelTurn {
        request_id: request_id.into(),
        session_id: "s1".into(),
        turn_id: "t1".into(),
    }
}

fn ended(turn_id: &str, outcome: TurnOutcome) -> SessionBody {
    SessionBody::TurnEnded {
        turn_id: turn_id.into(),
        outcome,
        stop_reason: None,
        error: None,
    }
}

/// `cancel_turn` is completed by its turn's end, whatever the outcome; a
/// fact about another turn or only about the session does not complete it.
#[tokio::test]
async fn a_turn_waiter_resolves_only_on_that_turns_end() {
    let hub = Arc::new(Hub::new());
    let (_conn, mut rx) = connect(&hub);
    let call = tokio::spawn({
        let hub = hub.clone();
        async move {
            hub.request_for_turn("h", "rc", "t1", cancel("rc"), Duration::from_secs(5))
                .await
        }
    });
    rx.recv().await.expect("the cancel went out");
    hub.resolve_turn("t0", ended("t0", TurnOutcome::Completed));
    hub.resolve_session(
        "s1",
        SessionBody::SessionParked {
            reason: ParkReason::Operator,
        },
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!call.is_finished(), "completed by a fact about something else");
    hub.resolve_turn("t1", ended("t1", TurnOutcome::Completed));
    assert_eq!(call.await.unwrap(), Ok(ended("t1", TurnOutcome::Completed)));

    // A rejection still matches the request id.
    let call = tokio::spawn({
        let hub = hub.clone();
        async move {
            hub.request_for_turn("h", "rc2", "t1", cancel("rc2"), Duration::from_secs(5))
                .await
        }
    });
    rx.recv().await.expect("the second cancel went out");
    hub.reject("rc2", "not_running".into(), "no".into());
    assert_eq!(
        call.await.unwrap(),
        Err(RequestError::Rejected {
            code: "not_running".into(),
            message: "no".into()
        })
    );
}

#[tokio::test]
async fn capabilities_belong_to_the_hosts_current_connection() {
    let hub = Hub::new();
    assert!(!hub.has_capability("h", Capability::Park), "an unknown host has none");
    let (tx, _rx) = mpsc::unbounded_channel();
    let first = hub.register("h", tx, Capabilities(vec![Capability::Park])).unwrap();
    assert!(hub.has_capability("h", Capability::Park));
    assert!(!hub.has_capability("h", Capability::Images));
    hub.unregister("h", first.conn_id);
    assert!(!hub.has_capability("h", Capability::Park), "a gone host has none");
    let (tx, _rx) = mpsc::unbounded_channel();
    hub.register("h", tx, Capabilities::default()).unwrap();
    assert!(
        !hub.has_capability("h", Capability::Park),
        "an older build of the host that cannot park reconnected"
    );
}
```

Append to `crates/hennery-testkit/tests/reconcile.rs`:

```rust
// Plan B2a: capabilities (ACP core §3.3).

/// A newer host may announce a capability this collector does not know: its
/// `hello` is still accepted, and the capabilities it shares are kept.
#[tokio::test]
async fn a_hello_with_an_unknown_capability_is_accepted_with_the_known_ones() {
    let collector = Collector::start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
        .await
        .unwrap();
    let hello = json!({
        "type": "hello", "protocol_version": PROTOCOL_VERSION, "host_version": "future",
        "host_id": HOST, "token": TOKEN, "capabilities": ["teleport", "park"], "attached_sessions": []
    });
    ws.send(Message::text(hello.to_string())).await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("an answer to hello")
        .unwrap()
        .unwrap();
    let ack: CollectorFrame = serde_json::from_str(ack.to_text().unwrap()).unwrap();
    assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
    assert!(collector.state.hub.has_capability(HOST, Capability::Park));
    assert!(!collector.state.hub.has_capability(HOST, Capability::Images));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-sessions --test hub`
Expected: compile error: `register` takes 2 arguments; no method `request_for_turn`, `resolve_turn`, `has_capability`.

- [ ] **Step 3: Implement**

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
use hennery_proto::frames::{CollectorFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{Capabilities, Capability, CollectorFrame, SessionBody};
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
struct Waiter {
    /// The connection the request went out on: only its loss or its
    /// timeout concerns this waiter.
    conn_id: u64,
    /// Set for requests completed by a fact that names only the session
    /// (`session_parked`, `session_closed`), not the request (§3.2).
    session_id: Option<String>,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}
```

with:

```rust
/// The fact that completes a request, besides a rejection of its
/// `request_id` (ACP core §3.2, §3.3).
enum CompletedBy {
    /// A fact that carries the `request_id` (`resolve`).
    Request,
    /// A fact that names only the session: `session_parked`,
    /// `session_closed` (`resolve_session`).
    Session(String),
    /// The turn's `turn_ended` (`resolve_turn`): `cancel_turn`.
    Turn(String),
}

struct Waiter {
    /// The connection the request went out on: only its loss or its
    /// timeout concerns this waiter.
    conn_id: u64,
    completed_by: CompletedBy,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    ready: bool,
    kicked: CancellationToken,
}
```

with:

```rust
    ready: bool,
    kicked: CancellationToken,
    /// From this connection's `hello` (ACP core §3.3).
    capabilities: Capabilities,
}
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    /// Register a host connection (not yet ready). A second live connection
    /// for the same host id is refused, never allowed to supersede the
    /// first silently.
    pub fn register(&self, host_id: &str, tx: mpsc::UnboundedSender<CollectorFrame>) -> Option<Registration> {
```

with:

```rust
    /// Register a host connection (not yet ready) with the capabilities its
    /// `hello` announced. A second live connection for the same host id is
    /// refused, never allowed to supersede the first silently.
    pub fn register(
        &self,
        host_id: &str,
        tx: mpsc::UnboundedSender<CollectorFrame>,
        capabilities: Capabilities,
    ) -> Option<Registration> {
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
                ready: false,
                kicked: kicked.clone(),
            },
```

with:

```rust
                ready: false,
                kicked: kicked.clone(),
                capabilities,
            },
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    /// Send a request and wait until the outboxed fact carrying `request_id`
```

with:

```rust
    /// The host's current connection announced `capability`. A host that is
    /// not connected has none.
    pub fn has_capability(&self, host_id: &str, capability: Capability) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .is_some_and(|h| h.capabilities.has(capability))
    }

    /// Send a request and wait until the outboxed fact carrying `request_id`
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        self.wait(host_id, request_id, None, frame, timeout).await
    }
```

with:

```rust
        self.wait(host_id, request_id, CompletedBy::Request, frame, timeout)
            .await
    }
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        self.wait(host_id, request_id, Some(session_id.to_string()), frame, timeout)
            .await
    }

    async fn wait(
        &self,
        host_id: &str,
        request_id: &str,
        session_id: Option<String>,
```

with:

```rust
        let completed_by = CompletedBy::Session(session_id.to_string());
        self.wait(host_id, request_id, completed_by, frame, timeout).await
    }

    /// Like `request`, for a request completed by the end of `turn_id`
    /// (`resolve_turn`), whatever its outcome. Rejections still match
    /// `request_id`.
    pub async fn request_for_turn(
        &self,
        host_id: &str,
        request_id: &str,
        turn_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let completed_by = CompletedBy::Turn(turn_id.to_string());
        self.wait(host_id, request_id, completed_by, frame, timeout).await
    }

    async fn wait(
        &self,
        host_id: &str,
        request_id: &str,
        completed_by: CompletedBy,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
                    conn_id: host.conn_id,
                    session_id,
                    tx,
```

with:

```rust
                    conn_id: host.conn_id,
                    completed_by,
                    tx,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    pub fn resolve_session(&self, session_id: &str, fact: SessionBody) {
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| w.session_id.as_deref() == Some(session_id))
```

with:

```rust
    pub fn resolve_session(&self, session_id: &str, fact: SessionBody) {
        self.resolve_where(|by| matches!(by, CompletedBy::Session(s) if s == session_id), fact);
    }

    /// Resolve every waiter registered with `request_for_turn` for `turn_id`.
    pub fn resolve_turn(&self, turn_id: &str, fact: SessionBody) {
        self.resolve_where(|by| matches!(by, CompletedBy::Turn(t) if t == turn_id), fact);
    }

    fn resolve_where(&self, completes: impl Fn(&CompletedBy) -> bool, fact: SessionBody) {
        let mut waiters = self.waiters.lock().expect("waiters lock");
        let ids: Vec<String> = waiters
            .iter()
            .filter(|(_, w)| completes(&w.completed_by))
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        token,
        attached_sessions,
        ..
    }) = hello
```

with:

```rust
        token,
        capabilities,
        attached_sessions,
        ..
    }) = hello
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    let Some(registration) = state.hub.register(&host_id, tx.clone()) else {
```

with:

```rust
    let Some(registration) = state.hub.register(&host_id, tx.clone(), capabilities) else {
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
                            SessionBody::SessionParked { .. } | SessionBody::SessionClosed => {
                                state.hub.resolve_session(&session_id, body.clone());
                            }
```

with:

```rust
                            SessionBody::SessionParked { .. } | SessionBody::SessionClosed => {
                                state.hub.resolve_session(&session_id, body.clone());
                            }
                            // `cancel_turn` is completed by its turn's end.
                            SessionBody::TurnEnded { turn_id, .. } => {
                                state.hub.resolve_turn(turn_id, body.clone());
                            }
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        .register("host-1", tx)
```

with:

```rust
        .register("host-1", tx, Default::default())
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-sessions --test hub && cargo test -p hennery-testkit --test reconcile`
Expected: all 4 hub tests pass, and all 20 reconcile tests pass, including `a_hello_with_an_unknown_capability_is_accepted_with_the_known_ones`.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-sessions crates/hennery-testkit/tests
git commit -m "feat(sessions): keep host capabilities and complete requests by a turn's end"
```

---

### Task 7: A host's rejection reaches the store without a waiting handler

**Files:**
- Modify: `crates/hennery-sessions/src/hub.rs`, `crates/hennery-sessions/src/ws.rs`, `crates/hennery-sessions/src/api.rs`
- Test: `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: `CompletedBy` and `wait` (Task 6).
- Produces: `pub enum hennery_sessions::hub::Undo { Start { session_id: String }, Prompt { session_id: String, turn_id: String } }` (`Debug, Clone, PartialEq`).
- Produces: `Hub::request_with_undo(&self, host_id, request_id, frame, timeout, undo: Undo) -> Result<SessionBody, RequestError>` and `Hub::undo_for(&self, request_id: &str) -> Option<Undo>`. `Hub::request` keeps its signature and registers no undo.
- Produces: on a host `error`, `ws.rs` applies the waiter's undo before `reject` signals it (decision 6):
  - `Start` → `Store::mark_failed_if_starting(session_id, code)`;
  - `Prompt` → `Store::abandon_turn(session_id, turn_id)`.
- Produces: `start_session`, `resume` and `prompt` send with `request_with_undo`. They write to the store themselves only when the request was never sent (`RequestError::NotConnected`).

The carry-over (B1 "Execution status"): when a client disconnects, hyper drops the in-flight handler future. The waiter entry stays in the hub, so the host's later rejection still finds it, but the handler that would have written `failed` (or removed the turn) is gone. While the host stays connected, nothing else resolves it: the session sits in `starting`, or every prompt gets 409 `turn_in_progress`. The tests reproduce this with a client that gives up after 300 ms. Checked on hyper 1.x: the handler is dropped when the client closes the connection.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/reconcile.rs`:

```rust
// Plan B2a: a host's rejection reaches the store even when the HTTP caller
// has given up (plan B1, "Execution status"). The client times out, hyper
// drops the handler, and only then does the host answer.

/// POST with a client that gives up after 300 ms; resolves once it has.
fn post_and_give_up(url: String, body: Value) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sent = client()
            .post(url)
            .json(&body)
            .timeout(Duration::from_millis(300))
            .send()
            .await;
        assert!(sent.is_err(), "the collector answered before the host did: {sent:?}");
    })
}

/// Reject `request_id` once its caller is gone.
async fn reject_after_the_caller_left(
    host: &mut ScriptedHost,
    caller: tokio::task::JoinHandle<()>,
    request_id: String,
) {
    caller.await.unwrap();
    // Let the server notice the closed connection and drop the handler.
    tokio::time::sleep(Duration::from_millis(200)).await;
    host.send(&HostFrame::Error {
        request_id,
        code: "unknown_agent".into(),
        message: "rejected".into(),
    })
    .await;
}

fn failed_with(collector: &Collector, session: &str) -> Option<String> {
    let row = collector.state.store.session(session).unwrap().unwrap();
    (row.lifecycle == "failed").then_some(row.failure_reason).flatten()
}

#[tokio::test]
async fn a_start_rejected_after_its_caller_gave_up_still_fails_the_session() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let caller = post_and_give_up(
        collector.url("/api/sessions"),
        json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp" }),
    );
    let CollectorFrame::StartSession {
        request_id, session_id, ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    let reason = wait_for("start failed", || async { failed_with(&collector, &session_id) }).await;
    assert_eq!(reason, "unknown_agent");
}

#[tokio::test]
async fn a_resume_rejected_after_its_caller_gave_up_still_fails_the_session() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let caller = post_and_give_up(resume_url(&collector, &session), json!({}));
    let request_id = expect_resume(&mut host, &session).await;
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    let reason = wait_for("resume failed", || async { failed_with(&collector, &session) }).await;
    assert_eq!(reason, "unknown_agent");
}

#[tokio::test]
async fn a_prompt_rejected_after_its_caller_gave_up_still_frees_the_turn_slot() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let caller = post_and_give_up(collector.url(&format!("/api/sessions/{session}/prompt")), prompt_body());
    let CollectorFrame::Prompt { request_id, .. } = host.next().await else {
        panic!("expected a prompt");
    };
    reject_after_the_caller_left(&mut host, caller, request_id).await;
    wait_for("turn slot free", || async {
        let row = collector.state.store.session(&session).unwrap().unwrap();
        row.open_turn_id.is_none().then_some(())
    })
    .await;
    // The next prompt is not refused `turn_in_progress`.
    started_turn(&collector, &mut host, &session).await;
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test reconcile gave_up`
Expected: all three FAIL with "timed out waiting for start failed" / "resume failed" / "turn slot free". The rejection found the waiter, but nobody wrote the store.

- [ ] **Step 3: Implement**

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
struct Waiter {
    /// The connection the request went out on: only its loss or its
    /// timeout concerns this waiter.
    conn_id: u64,
    completed_by: CompletedBy,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}
```

with:

```rust
/// What a request changed in the store before it was sent. A rejection
/// means nothing happened on the host, so the socket task undoes it when
/// the rejection arrives: it sees every rejection, even when the HTTP
/// handler that sent the request is gone (its client disconnected).
#[derive(Debug, Clone, PartialEq)]
pub enum Undo {
    /// A start or resume left the session `starting`: it becomes `failed`
    /// with the rejection's code.
    Start { session_id: String },
    /// A prompt holds the session's turn slot as `sent`: the turn is
    /// removed.
    Prompt { session_id: String, turn_id: String },
}

struct Waiter {
    /// The connection the request went out on: only its loss or its
    /// timeout concerns this waiter.
    conn_id: u64,
    completed_by: CompletedBy,
    undo: Option<Undo>,
    tx: oneshot::Sender<Result<SessionBody, RequestError>>,
}
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        self.wait(host_id, request_id, CompletedBy::Request, frame, timeout)
            .await
    }
```

with:

```rust
        self.wait(host_id, request_id, CompletedBy::Request, None, frame, timeout)
            .await
    }

    /// Like `request`, for a request whose store change `undo` reverts if
    /// the host rejects it (`undo_for`).
    pub async fn request_with_undo(
        &self,
        host_id: &str,
        request_id: &str,
        frame: CollectorFrame,
        timeout: Duration,
        undo: Undo,
    ) -> Result<SessionBody, RequestError> {
        self.wait(host_id, request_id, CompletedBy::Request, Some(undo), frame, timeout)
            .await
    }
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        let completed_by = CompletedBy::Session(session_id.to_string());
        self.wait(host_id, request_id, completed_by, frame, timeout).await
```

with:

```rust
        let completed_by = CompletedBy::Session(session_id.to_string());
        self.wait(host_id, request_id, completed_by, None, frame, timeout).await
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        let completed_by = CompletedBy::Turn(turn_id.to_string());
        self.wait(host_id, request_id, completed_by, frame, timeout).await
```

with:

```rust
        let completed_by = CompletedBy::Turn(turn_id.to_string());
        self.wait(host_id, request_id, completed_by, None, frame, timeout).await
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        completed_by: CompletedBy,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let (tx, rx) = oneshot::channel();
```

with:

```rust
        completed_by: CompletedBy,
        undo: Option<Undo>,
        frame: CollectorFrame,
        timeout: Duration,
    ) -> Result<SessionBody, RequestError> {
        let (tx, rx) = oneshot::channel();
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
                    conn_id: host.conn_id,
                    completed_by,
                    tx,
```

with:

```rust
                    conn_id: host.conn_id,
                    completed_by,
                    undo,
                    tx,
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    pub fn reject(&self, request_id: &str, code: String, message: String) {
```

with:

```rust
    /// The undo registered with a request still waiting for its answer.
    /// The waiter outlives an HTTP handler that was dropped mid-request:
    /// only its answer, the connection's loss or its timeout removes it.
    pub fn undo_for(&self, request_id: &str) -> Option<Undo> {
        self.waiters
            .lock()
            .expect("waiters lock")
            .get(request_id)
            .and_then(|w| w.undo.clone())
    }

    pub fn reject(&self, request_id: &str, code: String, message: String) {
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
use crate::AppState;
```

with:

```rust
use crate::AppState;
use crate::hub::Undo;
use crate::store::Store;
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
                } else {
                    state.hub.reject(&request_id, code, message);
                }
```

with:

```rust
                } else {
                    // Undo what the request changed before its waiter hears
                    // of the rejection, so the HTTP answer and the store
                    // agree, and the store is right even with no waiter.
                    if let Some(undo) = state.hub.undo_for(&request_id)
                        && let Err(err) = undo_rejected(&state.store, &undo, &code)
                    {
                        tracing::error!(%host_id, ?undo, error = %err, "undoing a rejected request failed");
                    }
                    state.hub.reject(&request_id, code, message);
                }
```

Append to `crates/hennery-sessions/src/ws.rs`:

```rust
/// Revert what a request the host rejected changed in the store: a start or
/// resume fails with the host's code, a prompt's turn is removed.
fn undo_rejected(store: &Store, undo: &Undo, code: &str) -> anyhow::Result<()> {
    match undo {
        Undo::Start { session_id } => store.mark_failed_if_starting(session_id, code),
        Undo::Prompt { session_id, turn_id } => store.abandon_turn(session_id, turn_id),
    }
}
```

The `StartFailed` path in `ws.rs` also calls `reject`. It applies no undo, because ingest has already made that fact's transition.

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use crate::hub::RequestError;
```

with:

```rust
use crate::hub::{RequestError, Undo};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    match state.hub.request(&req.host_id, &request_id, frame, START_TIMEOUT).await {
        Ok(_) => (StatusCode::ACCEPTED, Json(StartSessionResponse { session_id })).into_response(),
```

with:

```rust
    let undo = Undo::Start {
        session_id: session_id.clone(),
    };
    match state
        .hub
        .request_with_undo(&req.host_id, &request_id, frame, START_TIMEOUT, undo)
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(StartSessionResponse { session_id })).into_response(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        Err(err) => {
            let reason = match &err {
                RequestError::Rejected { code, .. } => code.clone(),
                _ => "host_offline".into(),
            };
            if let Err(e) = state.store.mark_failed(&session_id, &reason) {
                return internal(e);
            }
            request_failed(err)
        }
```

with:

```rust
        // Never sent.
        Err(RequestError::NotConnected) => {
            if let Err(e) = state.store.mark_failed(&session_id, "host_offline") {
                return internal(e);
            }
            request_failed(RequestError::NotConnected)
        }
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`).
        Err(err) => request_failed(err),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    match state
        .hub
        .request(&session.host_id, &request_id, frame, START_TIMEOUT)
        .await
    {
        Ok(_) => lifecycle_response(&state, &id),
        // Still `starting`: the next handshake reconciles it (ACP core §3.4).
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        Err(err) => {
            let reason = match &err {
                RequestError::Rejected { code, .. } => code.clone(),
                _ => "host_offline".into(),
            };
            if let Err(e) = state.store.mark_failed_if_starting(&id, &reason) {
                return internal(e);
            }
            resume_failed(err)
        }
    }
```

with:

```rust
    let undo = Undo::Start { session_id: id.clone() };
    match state
        .hub
        .request_with_undo(&session.host_id, &request_id, frame, START_TIMEOUT, undo)
        .await
    {
        Ok(_) => lifecycle_response(&state, &id),
        // Still `starting`: the next handshake reconciles it (ACP core §3.4).
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        // Never sent: the host went away since the check above.
        Err(RequestError::NotConnected) => {
            if let Err(e) = state.store.mark_failed_if_starting(&id, "host_offline") {
                return internal(e);
            }
            resume_failed(RequestError::NotConnected)
        }
        // The socket task has already failed the session with the host's
        // code (`Undo::Start`).
        Err(err) => resume_failed(err),
    }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    match state
        .hub
        .request(&session.host_id, &request_id, frame, PROMPT_TIMEOUT)
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(PromptResponse { turn_id })).into_response(),
        // Unknown delivery keeps the turn open; the outbox resolves it.
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        Err(err) => {
            if let Err(e) = state.store.abandon_turn(&id, &turn_id) {
                return internal(e);
            }
            request_failed(err)
        }
    }
```

with:

```rust
    let undo = Undo::Prompt {
        session_id: id.clone(),
        turn_id: turn_id.clone(),
    };
    match state
        .hub
        .request_with_undo(&session.host_id, &request_id, frame, PROMPT_TIMEOUT, undo)
        .await
    {
        Ok(_) => (StatusCode::ACCEPTED, Json(PromptResponse { turn_id })).into_response(),
        // Unknown delivery keeps the turn open; the outbox resolves it.
        Err(RequestError::DeliveryUnknown) => request_failed(RequestError::DeliveryUnknown),
        // Never sent.
        Err(RequestError::NotConnected) => {
            if let Err(e) = state.store.abandon_turn(&id, &turn_id) {
                return internal(e);
            }
            request_failed(RequestError::NotConnected)
        }
        // The socket task has already removed the turn (`Undo::Prompt`).
        Err(err) => request_failed(err),
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test reconcile && cargo test -p hennery-testkit --test e2e`
Expected: all 23 reconcile tests pass, including the three new ones. The existing `a_resume_the_host_rejects_is_a_502_with_its_code` still passes, so when the caller waits, the 502 and the stored failure agree. The e2e `a_start_that_fails_on_the_host_is_reported_as_502` still answers 502 `unknown_agent`.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-sessions/src crates/hennery-testkit/tests/reconcile.rs
git commit -m "fix(sessions): apply a host's rejection even when no handler waits"
```

---

### Task 8: `POST /api/sessions/{id}/cancel` and the park gate

**Files:**
- Modify: `crates/hennery-sessions/src/api.rs`
- Test: `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: `Hub::request_for_turn`, `Hub::has_capability` (Task 6); `CollectorFrame::CancelTurn`, `CancelResponse`, `Capability` (Task 1).
- Produces: `POST /api/sessions/{id}/cancel` (no body). Answers (decisions 1 and 4):
  - 202 `CancelResponse {turn_id, outcome}` once that turn's `turn_ended` is ingested;
  - 409 `not_attached` when the session is not `active` or its host is not ready;
  - 409 `no_open_turn`;
  - 409 `not_running` when the host rejects it;
  - 404 for an unknown session;
  - 503 `delivery_unknown`.
- Produces: `request_failed` maps `not_running` to 409.
- Produces: `POST …/park` answers 409 `park_unsupported`, and sends nothing, when the host's connection did not announce `park` (decision 5).
- Test support: `ScriptedHost::hello_with` / `connect_with` take the capabilities to announce. `hello` / `connect` announce `park`, as before.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    /// Connect and complete `hello` / `hello_ack`, without `resend_complete`.
    async fn hello(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
```

with:

```rust
    /// Connect and complete `hello` / `hello_ack`, without `resend_complete`.
    async fn hello(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        Self::hello_with(collector, attached, seq, Capabilities(vec![Capability::Park])).await
    }

    /// `hello` announcing `capabilities`.
    async fn hello_with(
        collector: &Collector,
        attached: Vec<AttachedSession>,
        seq: u64,
        capabilities: Capabilities,
    ) -> Self {
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            capabilities: Capabilities(vec![Capability::Park]),
            attached_sessions: attached,
```

with:

```rust
            capabilities,
            attached_sessions: attached,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    async fn connect(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        let mut host = Self::hello(collector, attached, seq).await;
```

with:

```rust
    async fn connect(collector: &Collector, attached: Vec<AttachedSession>, seq: u64) -> Self {
        Self::connect_with(collector, attached, seq, Capabilities(vec![Capability::Park])).await
    }

    /// `connect` announcing `capabilities`.
    async fn connect_with(
        collector: &Collector,
        attached: Vec<AttachedSession>,
        seq: u64,
        capabilities: Capabilities,
    ) -> Self {
        let mut host = Self::hello_with(collector, attached, seq, capabilities).await;
```

Append to `crates/hennery-testkit/tests/reconcile.rs`:

```rust
// Plan B2a: `POST /api/sessions/{id}/cancel` and the park gate.

fn cancel_url(collector: &Collector, session: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/cancel"))
}

/// The next frame must be a `cancel_turn` for `turn`; returns its request id.
async fn expect_cancel(host: &mut ScriptedHost, session: &str, turn: &str) -> String {
    match host.next().await {
        CollectorFrame::CancelTurn {
            request_id,
            session_id,
            turn_id,
        } => {
            assert_eq!((session_id.as_str(), turn_id.as_str()), (session, turn));
            request_id
        }
        other => panic!("expected cancel_turn, got {other:?}"),
    }
}

fn turn_ended(turn: &str, outcome: hennery_proto::frames::TurnOutcome) -> SessionBody {
    SessionBody::TurnEnded {
        turn_id: turn.into(),
        outcome,
        stop_reason: None,
        error: None,
    }
}

#[tokio::test]
async fn a_cancel_ends_the_open_turn_and_answers_with_its_outcome() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    expect_cancel(&mut host, &session, &turn).await;
    host.emit(
        &session,
        turn_ended(&turn, hennery_proto::frames::TurnOutcome::Cancelled),
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(status, 202, "{body}");
    assert_eq!(body, json!({ "turn_id": turn, "outcome": "cancelled" }));
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!((row.open_turn_id, row.activity.as_deref()), (None, Some("idle")));
}

/// The turn finished just before the cancel reached the host: the host's
/// `turn_ended{completed}` is on the wire ahead of its `not_running`
/// rejection, and the cancel answers with that outcome.
#[tokio::test]
async fn a_cancel_that_loses_the_race_with_the_turns_end_answers_how_it_ended() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.emit(
        &session,
        turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
    )
    .await;
    host.send(&HostFrame::Error {
        request_id,
        code: "not_running".into(),
        message: "that turn is not running".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!(
        (status, body),
        (202, json!({ "turn_id": turn, "outcome": "completed" }))
    );
}

#[tokio::test]
async fn a_cancel_is_refused_without_a_turn_the_host_runs() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(), cancel_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    let (status, _) = post(&client(), cancel_url(&collector, "no-such-session"), json!({})).await;
    assert_eq!(status, 404);

    // The host has no such turn in flight.
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.send(&HostFrame::Error {
        request_id,
        code: "not_running".into(),
        message: "that turn is not running".into(),
    })
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("not_running")));

    let parked = parked_session(&collector, &mut host).await;
    let (status, body) = post(&client(), cancel_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
}

/// ACP core §3.3: `park_session` goes only to hosts with the `park`
/// capability (this drops plan A's decision 12).
#[tokio::test]
async fn park_goes_only_to_a_host_that_announced_it_can_park() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect_with(&collector, vec![], 0, Capabilities::default()).await;
    let session = started_session(&collector, &mut host).await;
    let park_url = collector.url(&format!("/api/sessions/{session}/park"));
    let (status, body) = post(&client(), park_url.clone(), json!({})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("park_unsupported")),
        "{body}"
    );
    assert_eq!(collector.lifecycle(&session), "active");
    let more = tokio::time::timeout(Duration::from_millis(300), host.next()).await;
    assert!(more.is_err(), "park reached a host that cannot park: {more:?}");

    // The same host, upgraded: now it is asked.
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let c = client();
    let call = tokio::spawn(async move { post(&c, park_url, json!({})).await });
    assert!(matches!(host.next().await, CollectorFrame::ParkSession { .. }));
    host.emit(
        &session,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Operator,
        },
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("parked")), "{body}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test reconcile -- cancel park_goes`
Expected: the three cancel tests FAIL. There is no such route yet, so the answer is 404 and no `cancel_turn` reaches the host ("a collector frame within 10s"). `park_goes_only_to_a_host_that_announced_it_can_park` FAILS after 15 s: the park reaches the host that cannot park, the scripted host never answers it, and the client times out.

- [ ] **Step 3: Implement**

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use hennery_proto::frames::CollectorFrame;
use hennery_proto::rest::{
    ApiError, EventDto, LifecycleResponse, OpenTurn, PromptRequest, PromptResponse, SessionDetail, StartSessionRequest,
    StartSessionResponse,
};
```

with:

```rust
use hennery_proto::frames::{Capability, CollectorFrame, SessionBody};
use hennery_proto::rest::{
    ApiError, CancelResponse, EventDto, LifecycleResponse, OpenTurn, PromptRequest, PromptResponse, SessionDetail,
    StartSessionRequest, StartSessionResponse,
};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// `park_session` / `close_session` (ACP core §3.4).
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(60);
```

with:

```rust
/// `park_session` / `close_session` (ACP core §3.4).
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(60);
/// `cancel_turn` (ACP core §3.4). The host stops an adapter that ignores the
/// cancel well before this (`hennery_host::session::CANCEL_GRACE`).
const CANCEL_TIMEOUT: Duration = Duration::from_secs(60);
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        && TEARDOWN_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
```

with:

```rust
        && TEARDOWN_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && CANCEL_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .route("/api/sessions/{id}/prompt", post(prompt))
```

with:

```rust
        .route("/api/sessions/{id}/prompt", post(prompt))
        .route("/api/sessions/{id}/cancel", post(cancel))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
                "not_attached" | "turn_in_progress" => StatusCode::CONFLICT,
```

with:

```rust
                "not_attached" | "turn_in_progress" | "not_running" => StatusCode::CONFLICT,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

with:

```rust
/// Cancel the open turn (ACP core §9): 202 `CancelResponse` once that
/// turn's `turn_ended` is ingested, with the outcome it really had: a turn
/// that finished before the cancel reached the agent is not `cancelled`.
async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" || !state.hub.is_ready(&session.host_id) {
        return error(StatusCode::CONFLICT, "not_attached", "the session is not attached");
    }
    let Some(turn_id) = session.open_turn_id else {
        return error(StatusCode::CONFLICT, "no_open_turn", "no turn is in flight");
    };
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::CancelTurn {
        request_id: request_id.clone(),
        session_id: id.clone(),
        turn_id: turn_id.clone(),
    };
    match state
        .hub
        .request_for_turn(&session.host_id, &request_id, &turn_id, frame, CANCEL_TIMEOUT)
        .await
    {
        Ok(SessionBody::TurnEnded { turn_id, outcome, .. }) => {
            (StatusCode::ACCEPTED, Json(CancelResponse { turn_id, outcome })).into_response()
        }
        Ok(other) => internal(anyhow::anyhow!("cancel completed by {other:?}")),
        // `not_running`: the host has no such turn in flight (409).
        Err(err) => request_failed(err),
    }
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        return error(StatusCode::CONFLICT, "not_attached", "the session is not attached");
    }
    match state.store.record_park_request(&id) {
```

with:

```rust
        return error(StatusCode::CONFLICT, "not_attached", "the session is not attached");
    }
    // Only to hosts that announced it (ACP core §3.3).
    if !state.hub.has_capability(&session.host_id, Capability::Park) {
        return error(
            StatusCode::CONFLICT,
            "park_unsupported",
            "this host cannot park sessions; close the session instead",
        );
    }
    match state.store.record_park_request(&id) {
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test reconcile`
Expected: all 27 pass.

- [ ] **Step 5: Lint and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/hennery-sessions/src/api.rs crates/hennery-testkit/tests/reconcile.rs
git commit -m "feat(sessions): add POST /api/sessions/{id}/cancel and gate park on the park capability"
```

---

### Task 9: End to end: cancel over a real host and adapter

**Files:**
- Test: `crates/hennery-testkit/tests/e2e.rs`

**Interfaces:**
- Consumes: everything above. The park gate is already covered end to end: `park_then_close_through_the_api_kill_the_adapters_whole_group` parks through a real host, which now passes only because the host announces `park` (Task 1) and the collector keeps it (Task 6).

- [ ] **Step 1: Write the test**

Append to `crates/hennery-testkit/tests/e2e.rs`:

```rust
// Plan B2a: cancel end to end (ACP core §3.3 `cancel_turn`, §4.4).

#[tokio::test]
async fn a_cancel_mid_turn_ends_it_cancelled_once_and_the_next_prompt_runs() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &slow_script(20));
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(&c, url, json!({ "content": text("long") })).await;
    assert_eq!(status, 202, "{body}");
    let turn = body["turn_id"].as_str().unwrap().to_string();

    let cancel_url = collector.url(&format!("/api/sessions/{session}/cancel"));
    let (status, body) = post_json(&c, cancel_url.clone(), json!({})).await;
    assert_eq!(
        (status, body),
        (202, json!({ "turn_id": turn, "outcome": "cancelled" }))
    );
    let evs = events(&c, &collector, &session).await;
    let ends = turn_ends(&evs);
    assert_eq!(ends.len(), 1, "{evs:?}");
    assert_eq!(
        (&ends[0].body["turn_id"], &ends[0].body["outcome"]),
        (&json!(turn), &json!("cancelled"))
    );
    assert!(
        agent_text(&evs).len() < "1.2.3.4.5.6.7.8.9.10.11.12.13.14.15.16.17.18.19.20.".len(),
        "the turn ran to its end"
    );
    // Nothing left to cancel; the session takes the next prompt.
    let (status, body) = post_json(&c, cancel_url, json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    lifecycle_is(&collector, &session, "active").await;
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(&c, url, json!({ "content": text("again") })).await;
    assert_eq!(status, 202, "{body}");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p hennery-testkit --test e2e a_cancel_mid_turn`
Expected: PASS in about a second. The 20-chunk turn (200 ms per chunk) is cut short and ends `cancelled` once, and the second prompt runs. If it fails, the task that owns the failing piece is wrong; do not patch it here.

- [ ] **Step 3: Full gate and commit**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check
git add crates/hennery-testkit/tests/e2e.rs
git commit -m "test(e2e): cancel a running turn over a real host and adapter"
```

Expected: 192 tests pass in the workspace.

---

## After this plan

**Plan B2b: model, axes and mode** (ACP core §3.2, §3.3, §4.3 step 4, §8, §9, §12 scenario 1). It builds on B1's `attach` path and on this plan's hub:
- **Wire.**
  - `start_session` / `resume_session` gain `model?`, `mode?` and `axes{}`.
  - `session_started` gains `indexed` (the catalogue extracts).
  - New `set_config {session_id, config_id, value}` and `config_applied {request_id, indexed}`.
  - `Indexed` gains `config_options`, `current_model` and `current_mode`.
  - REST: `StartSessionRequest` gains `model?`, `mode?`, `axes?`; new `ConfigRequest {config_id, value}`.
  - **Churn warning:** `SessionBody::SessionStarted` is built or destructured at about 28 sites (host actor, store, ws, and the store, reconcile, host_session, host_connection and ws_ingest_error tests). `StartSession` / `ResumeSession` literals sit at about 12 more. Start B2b with a task that adds `..` to every destructuring pattern and a test-support constructor for the literals. After that, the field additions are one-line changes, and every later task can be replayed in isolation.
- **Host.**
  - After `session/new` or `session/load`, apply model first, then the other axes, then mode, each with `session/set_config_option` (§4.3 step 4). Keep the catalogue the last successful switch returned, and announce it in `session_started` (the post-switch catalogue, P-13).
  - A failed switch on a resume is a `host_note{note: "reapply_failed"}` and does not fail the resume. Whether a failed switch fails a fresh start is a decision for that plan.
  - B1's frame order leaves room: the switches go between the load and `session_started`.
  - Codex's legacy `session/set_model` is sent as `UntypedMessage` (§2.4).
- **Fake adapter.** `session/set_config_option`, with a scripted catalogue in `session/new` / `session/load` answers and an option whose switch clamps the mode (§12 scenario 1). Also a scripted switch failure.
- **Collector.**
  - A migration for `model`, `mode`, `config_axes` and `session_catalog`.
  - Fill them from `session_started` / `config_applied` extracts only (§3.2, §8).
  - Re-send the stored values in `resume_session`.
  - `POST …/config` (409 `not_attached`, result via SSE `catalog_changed`) and `GET …/catalog`.
- **Live gates** (§12): model switch read-back; resume re-applies mode.

**Obligations B2a hands on:**
- **Cancel and pending requests.** `cancel_turn` must also resolve the turn's pending permissions and elicitations with `pending_resolved{cancelled, reason: turn_cancelled}` (§4.6). This lands with permission and elicitation.
- **`images` and `projects` capabilities.** The host announces them only once image prompts (with §7's collector validation and storage) and the project probes exist. The collector then refuses an image prompt to a host without `images`, and the probes to one without `projects`.
- **The unanswered-cancel park does not set the handle's `ending` mark.** It ends the actor by itself, like an idle reap or an adapter exit. A resume that arrives during that actor's kill grace is answered `not_attached`, so the session becomes `failed` (it can be resumed again). This is the same class as B1's "Operator park and close only" obligation, and it has the same fix: mark the handle from inside the actor.
- **Cancel grace is not configurable.** `CANCEL_GRACE` (20 s) is a constant, and `HostConfig` does not expose it. If real adapters need longer, expose it next to `idle_timeout`, keeping grace plus kill grace below the collector's 60 s.
- **A narrow cancel race.** A turn whose `turn_ended` is ingested after the handler read `open_turn_id`, but before its waiter was registered, answers 409 `not_running` instead of 202 with the real outcome. The window is between two lines of `cancel`. If the UI ever shows it, look the ended turn's outcome up on `not_running`.

Then, in order (unchanged from plan B1):
- **(2) Permission and elicitation.** The pending set and the answer queue, including the teardown hooks plan A left out and `turn_cancelled` above.
- **(3) Real auth and pairing.**
- **(4) Frontend shell.** Settle the generated TS optionals. Offer close for a `starting` session whose host is offline (B1). Hide park for hosts without the capability (§3.3).
- **(5) Hats.**
- **(6) Gateway.**
- **(7) Distribution.**

---

_Generated with Claude AI — please review before distribution._
