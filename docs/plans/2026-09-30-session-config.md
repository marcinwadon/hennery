# Model, axes and mode (plan B2b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A session starts with the model, mode and other config axes the operator picked, and keeps them. A resume re-applies what the agent last reported, and the operator can switch any option on an attached session. What hennery shows is always the agent's own read-back, never the request, and a switch that does not take never costs the operator the session.

**Architecture:**
- **Wire.** A `SessionConfig { model?, mode?, axes{} }` is flattened into `start_session`, `resume_session` and `POST /api/sessions`, so the JSON stays flat as in ACP core §3.3. New frames: `set_config` and `config_applied`. The catalogue extracts ride on `session_started`, on `config_applied` and on live `acp_update`s, and they always travel as one snapshot.
- **Host.** After `session/new` or `session/load`, the actor applies the model, then the other axes, then the mode, each with `session/set_config_option`. It keeps the catalogue the last switch answered with and announces it in `session_started`. A switch that fails is a `host_note`, never a failed start. On an attached session, `set_config` becomes one more switch, answered by `config_applied` or an error. The actor tracks the catalogue from switches and from the agent's own `config_option_update`s.
- **Collector.** The store writes the catalogue and its current values only from extracts: `session_catalog`, plus `sessions.model`, `mode` and `config_axes`. A resume re-sends those stored values. `POST …/config`, `GET …/catalog` and SSE `catalog_changed` sit on top.
- **Two B2a carry-overs.** An actor that ends by itself marks its handle ending, and a cap on updates in a row keeps a flooding adapter from holding off a cancel.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, agent-client-protocol 2.2.0 / schema 1.9.1 (`SetSessionConfigOptionRequest`, `SessionConfigOption`, `ConfigOptionUpdate`), rusqlite 0.40, schemars/ts-rs codegen. No new dependencies. Nix flake dev shell.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md). The relevant sections are:
- §2.4 (`session/set_config_option` through the crate's typed sender; methods absent from the schema as `UntypedMessage`);
- §3.2 (the catalogue extracts `config_options`, `current_model`, `current_mode`; the post-switch catalogue in `session_started`; `config_applied`);
- §3.3 (`start_session` / `resume_session` carry `model?, mode?, axes{}`; `set_config` → `config_applied` | `error`);
- §3.4 (the 60 s `set_config` timeout);
- §4.2 (config on a session that is not attached → 409 `not_attached`);
- §4.3 (model first, then the other axes, then mode; keep the catalogue of the last successful switch; a failed re-apply is a `host_note` and does not fail the resume; P-12, P-13);
- §8 (`session_catalog`; the `model`, `mode` and `config_axes` columns, from extracts only);
- §9 (`POST …/config`, `GET …/catalog`, SSE `catalog_changed`);
- §12 (scenario 1; the live gates "model switch read-back" and "resume re-applies mode", as far as the fake adapter can stand in for a real one).

It builds on the executed [cancel and capabilities plan](2026-09-29-cancel-capabilities.md) (plan B2a). Read its "Execution status" and "After this plan" first. Its code wins over its task text, and every anchor below was taken from that code (`main` at `75b2fc0`).

**Status:** not executed. Every code block below was built and tested in a scratch copy of `75b2fc0`. The plan was then replayed from its own text, task by task, onto a fresh copy of `75b2fc0`, with fmt, clippy, the workspace tests and the codegen check after every task (241 tests at the end, from 195). The decisions were reviewed on 2026-09-30, and amended as marked.

## Scope

This is **plan B2b**, the config half of B2 as B2a's "After this plan" scoped it. It fits in ten right-sized tasks, including both B2a carry-overs, so it is not split further.

**In:**
- Wire:
  - `ConfigValue`, `SessionConfig`, the catalogue extracts on `Indexed`, `session_started.indexed`, `config_applied`;
  - `config` on `start_session` / `resume_session`, and `set_config`;
  - REST: the config on `StartSessionRequest`, plus `ConfigRequest` and `SessionCatalog`.
- Fake adapter: config options in `session/new` / `session/load`, and `session/set_config_option` that validates like a real adapter. Also a model switch that clamps the mode, a mode the agent changes by itself, a switch log, an empty read-back and a hung switch.
- Host:
  - the start's and the resume's switches, in order and bounded;
  - the post-switch catalogue in `session_started`; the `config_failed` / `reapply_failed` notes;
  - `set_config` on an attached session;
  - live `config_option_update` extracts;
  - no extracts on replayed or pre-start updates.
- Collector:
  - migration 5 (`model`, `mode`, `config_axes`, `session_catalog`) and the catalogue from extracts only;
  - the stored config re-sent on resume;
  - `POST …/config`, `GET …/catalog`, SSE `catalog_changed`.
- B2a carry-overs: the actor marks its handle ending when it ends by itself; a per-iteration cap on updates in the actor's biased select.

**Out** (later plans; see "After this plan"):
- Codex's legacy `session/set_model` and the legacy `modes` / `session/set_mode`: they belong to adapter profiles, and there are none yet (decision 9);
- `host_agent_catalog` and `hello.agents[].catalog` (the New-session pickers before a session exists);
- commands, plan and usage in the catalogue;
- permission and elicitation; the `images` and `projects` capabilities; real auth; hats; gateway; frontend; distribution.

**Where B2a's hand-offs land:**

| B2a "After this plan" / "Execution status" | Here |
|---|---|
| Churn warning: `..` in every `SessionStarted` / `StartSession` / `ResumeSession` pattern, and one constructor for the literals | Task 1 |
| `start_session` / `resume_session` gain `model?`, `mode?`, `axes{}`; `session_started` gains `indexed`; `set_config` / `config_applied`; `Indexed` gains the catalogue extracts; REST `StartSessionRequest` config and `ConfigRequest` | Task 2 |
| Fake `session/set_config_option`, a scripted catalogue, an option whose switch clamps the mode, a scripted switch failure | Task 3 |
| Model, then axes, then mode after new/load; the catalogue of the last successful switch in `session_started`; `host_note{reapply_failed}` on a resume; whether a failed switch fails a fresh start (decision 2) | Task 4 |
| Codex's legacy `session/set_model` as `UntypedMessage` | **Later** (decision 9) |
| Migration for `model`, `mode`, `config_axes`, `session_catalog`, filled from extracts only; re-send the stored values in `resume_session` | Task 6 |
| `POST …/config` (409 `not_attached`, SSE `catalog_changed`) and `GET …/catalog` | Task 7 |
| Live gates: model switch read-back; resume re-applies mode | Task 8 (against the fake) |
| The unanswered-cancel park does not set the handle's ending mark | Task 9 |
| A flooding adapter can delay `Cancel` and its grace (Execution status) | Task 10 |
| Cancel and pending requests; `images` / `projects`; cancel grace not configurable | Unchanged, see "After this plan" |

## Decisions this plan makes where the spec is silent

Reviewed and confirmed (with the amendments above) on 2026-09-30 by a stronger-model review on the maintainer's behalf. The review checked them against agent-client-protocol-schema 1.9.1, claude-agent-acp 0.81.0 and codex-acp 1.7.0, confirmed decisions 3, 5, 7, 8, 9, 10 and 11 as written, and amended decisions 1, 2, 4 and 6 (marked **Amended** below). What it found:
- The TypeScript SDK the pinned adapters use handles JSON-RPC requests concurrently: it does not await one request's handler before reading the next.
- An agent offers boolean options only to a client that advertises `clientCapabilities.session.configOptions.boolean`; to any other it offers an `on` / `off` select instead (Claude turns `fast` into one).
- Both pinned adapters return `configOptions` on `session/new` and `session/load`.

The tasks implement the decisions as written here.

1. **Axes are ACP config options, and model and mode are two of them.**
   - The model is the option in category `model`. Without one, it is the option whose id is `model`, but only if that option has no category or a custom (`_`-prefixed) one: an option another category claims (even one this build does not know) is not the model. The same goes for `mode`. ACP makes categories a UX hint only, so an adapter without them must still work. **Amended.**
   - The host's `initialize` advertises `clientCapabilities.session.configOptions.boolean = {}`, so agents offer boolean options as booleans. **Amended.**
   - `axes` holds every other option, by config id. A value is a select's value id or a boolean (`ConfigValue`, an untagged `bool | string`).
   - `SessionConfig { model?, mode?, axes{} }` is `#[serde(flatten)]`ed into the frames and the start request, so the wire stays §3.3's flat `model?, mode?, axes{}`.
   - A requested model or mode with no option to match is a failed switch, reported like any other (decision 2). It is never dropped silently.
2. **A switch that does not take never fails a start or a resume.**
   - Start and resume apply the same way: the model, then each axis (in config-id order), then the mode, each with `session/set_config_option`. A value that is already current is not sent.
   - Each switch gets `CONFIG_TIMEOUT` (15 s), and none runs past the start's own 75 s deadline. So the collector's 90 s start timeout still fires after the host's answer, never before it.
   - **Amended.** Once a switch gets no answer in time, or the start deadline has passed, no further switch is sent for that start. The adapter may still apply the late switch, and it handles requests concurrently, so a late model switch could clamp a mode sent after it: model first and mode last would silently break. The rest are listed as `not sent: an earlier switch did not answer` (or `not sent: the start deadline passed`). A switch the deadline cut off says `no answer before the start deadline`, not `no answer within 15s`.
   - **Amended.** After the switches, each requested value is compared with the final read-back. A mismatch (the agent accepted a value and reports another, or a later switch undid it) is a failure too: `effort: asked high, agent reports low`.
   - Every failure lands in one `host_note` after `session_started`: a refused value, an option the adapter does not offer, a switch with no answer in time, one not sent, a mismatch. The code is `config_failed` on a start and `reapply_failed` on a resume.
   - The spec leaves the fresh-start case open. A start is not failed either: the picker's value may be stale (another pin, another host), and the note plus the real current values tell the operator what to change. Failing would throw the started adapter away for a setting the operator can fix in one click.
3. **An empty or missing read-back is not a catalogue.**
   - The crate parses `configOptions` leniently, so an answer that does not parse arrives as an empty list, not as an error. The host treats an empty list, or a switch that timed out, as "the current values are unknown". Until the adapter reports its options again, it announces no catalogue at all: `session_started` or `config_applied` then has no extracts.
   - The collector likewise ignores a snapshot with an empty option list.
   - Either way the stored model and mode survive. A resume then re-applies them, instead of storing a guess and re-applying the guess (P-13 through another door).
4. **The catalogue extracts are one snapshot, and they come only from facts newer than the switches.**
   - The extracts are `config_options` (the adapter's option objects, re-serialized from the crate's types), `current_model`, `current_mode`, and a new `current_axes`. When `config_options` is present and non-empty, the four describe the same moment.
   - `current_axes` refines §3.2's closed list. Without it, the collector would have to read ACP option objects to learn the other axes, which §3.2 forbids.
   - The host fills them on `session_started`, on `config_applied`, and on live `config_option_update` notifications (the agent changing its own config, e.g. leaving plan mode).
   - It never fills them on updates replayed by `session/load`, nor on updates the adapter sent before its `session/new` answer or while the start's switches ran. Those are older than the catalogue `session_started` announces (P-13).
   - **Amended.** If the `session/new` or `session/load` answer has no (or an empty) `configOptions`, the pre-switch catalogue is seeded from the last non-empty `config_option_update` the adapter sent before that answer. That update itself still carries no extracts.
   - `config_options` holds the options hennery can parse: the crate skips an option it cannot read.
5. **The stored config is what the agent last reported, not what was asked for.**
   - `sessions.model`, `mode` and `config_axes` hold the current values of the last snapshot, and `session_catalog.config_options` holds the options. Every snapshot that applies overwrites them.
   - A resume re-sends them whole, and the host skips whatever is already current. So a mode the agent chose itself mid-turn survives a host restart, and a mode an adapter clamped is not forced back.
6. **`set_config` on the host:**
   - The option must exist in the actor's catalogue (else `unknown_option`), and the value must be of the option's kind (else `invalid`). Whether a select offers the value is the adapter's call: its refusal, or no answer within `CONFIG_TIMEOUT`, is `config_failed`.
   - **Amended.** The actor queues `set_config` requests and sends at most one `session/set_config_option` at a time: the next goes out only once the previous one is answered or has timed out. The adapter handles requests concurrently, so two switches in flight could land in either order. Each request's deadline is its receipt on the host plus `CONFIG_TIMEOUT`: one still waiting when it passes is answered `config_failed` (`an earlier switch is still out`), one sent and unanswered by then `config_failed` (`no answer within …`).
   - Switches may run during a turn, and are answered in the order they came.
   - A switch still out when the actor ends is answered `not_attached`, after the actor's last fact, so the collector never sits out its 60 s timeout for it.
   - A `config_applied` that reaches the store after the session detached is stored but not applied (plan B1 decision 7).
7. **The REST and SSE surface:**
   - `POST …/config` waits for `config_applied` (60 s, §3.4) and answers 202 with the session's `SessionCatalog`. It refuses with the host's code: 409 `not_attached` / `unknown_option`, 400 `invalid`, 502 `config_failed`. A value that is neither a string nor a boolean is refused by the JSON extractor (422).
   - `GET …/catalog` answers the config options and current values. Commands, plan and usage join it with the plans that produce them.
   - SSE `catalog_changed` follows every listed event that carries a snapshot. It has the same `id:`, and its data is a `SessionCatalog`. It is derived from the stored event, so a replay from `Last-Event-ID` sends it too, as §9 asks of every SSE message.
8. **`session_catalog` holds only `config_options` for now.** Its `commands` and `usage` columns come with the plans that fill them.
9. **No legacy `session/set_model` yet.** §2.4 says how to send it (as `UntypedMessage`), but choosing it per adapter is adapter-profile behaviour (§6), and hennery has no profiles yet: an agent is a command line. The Codex profile takes it on, along with reading the crate's `models` / `modes` into the extracts.
10. **An actor that ends by itself marks its handle ending when it begins to end.** That covers an idle reap, an adapter exit, and an adapter stopped for ignoring a cancel. A start or resume that arrives while it kills its adapter waits for it and attaches a fresh one, as it already does behind an operator park or close. It is no longer routed to the ending actor and answered `not_attached`.
11. **At most 64 updates in a row (`UPDATE_BURST`), then the actor's other arms get a turn.**
    - After a burst, the biased select's other arms get one pass (commands, the prompt's reply, the cancel grace). A `ready()` arm ends the pass when none of them is ready.
    - The ordering drains take only what is queued when they start, so a flood cannot hold them either.
    - A prompt drains what is queued before it starts its turn, so older updates are never tagged with the new turn.

**Spec drift to reconcile after review:** these are refinements of ACP core §3.2, §3.3 and §9, and the spec text should be amended to match:
- the `current_axes` extract;
- the `SessionCatalog` shape;
- the codes `unknown_option` / `config_failed` and the notes `config_failed` / `reapply_failed`, with their line texts `not sent: an earlier switch did not answer` and `asked X, agent reports Y`;
- 202 with the catalogue on `POST …/config`;
- the host advertises `session.configOptions.boolean` in `initialize` (§2.5, §6);
- one `session/set_config_option` at a time per session (§4.3, §3.3);
- `config_options` extracts are "the options hennery can parse" (§3.2).

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- `cargo fmt --all --check` (`max_width = 120`) and `cargo clippy --workspace --all-targets --locked -- -D warnings` pass after every task; `cargo test --workspace --locked` passes after every task.
- Generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated with `cargo run -p hennery-proto --bin gen` whenever a wire type changes and must pass `cargo run -p hennery-proto --bin gen -- --check`. In `codegen.rs`, new root types (frames, REST payloads) go in both `add!` lists and new nested types go in the `render_ts` list.
- ACP payloads are forwarded verbatim as `serde_json::Value` and never scrubbed; stderr tails and `host_note` text are scrubbed (ACP core §2.3).
- Every state-bearing fact from the host is a sequenced `session` frame through the outbox; only rejections (`error`) bypass it (ACP core §3.3). `config_applied` is such a fact.
- "`session_catalog`, `plans` and the `model`/`mode` columns (§8) are filled **from extracts only**" (ACP core §3.2).
- The host "applies **model first, then other axes, then mode**, each via `session/set_config_option`, keeping the catalogue returned by the last successful switch" and "emits `session_started` with that catalogue" (ACP core §4.3).
- "A failed re-apply is logged on the timeline as a `host_note` and does not fail the resume" (ACP core §4.3).
- "**Prompt or config on a session that is not attached** (parked, closed, failed, or its host offline) → 409 `not_attached`" (ACP core §4.2).
- Collector timeouts: start and resume 90 s, prompt 60 s, `set_config`, `cancel_turn`, `park_session`, `close_session` 60 s, all ≥ the 45 s read deadline (ACP core §3.4).
- "Heavy blobs never ride the list": catalogues have their own table and endpoint (ACP core §8).
- The collector acks a frame only after its transaction commits; ingest is idempotent on `(session_id, seq)`.
- The collector sends no request to a host before that connection's post-`resend_complete` reconciliation (ACP core §5.1 step 4).
- No global installs: tooling comes from the flake dev shell.
- Commits: Conventional Commits (`feat(host): …`), made with the repository's own identity (gmail, unsigned); push the feature branch after every completed task; never push `main`.

## Review Focus

These are the five inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **A stale or bogus value from the New-session picker** (a model another pin offered, an option this adapter lacks). Expected: the session still starts, and a `host_note{config_failed}` says what did not take. The values shown are the adapter's real ones. The refused model is never reported as current, not on start and not after a later `set_config`. (Task 4: `a_start_whose_switches_fail_still_starts_and_says_why`; Task 8: `a_model_switch_answers_with_the_adapters_read_back_and_a_bogus_model_is_never_current`)
2. **The agent changes its own mode mid-turn** (leaves plan mode). Expected: the live `config_option_update` carries the catalogue, and the store keeps the new mode. After a host restart, the resume applies it to the fresh adapter, which starts from its default. (Task 5: `only_a_live_config_option_update_carries_the_catalogue`; Task 8: `a_mode_the_agent_chose_survives_a_host_restart_and_resume`)
3. **A resume whose `session/load` replays an old `config_option_update`,** or an adapter that sends updates while the start's switches run. Expected: those updates carry no catalogue, so the post-switch catalogue in `session_started` is what is stored (P-13). (Task 5: `only_a_live_config_option_update_carries_the_catalogue`; Task 6: `a_config_applied_or_a_live_update_replaces_the_catalogue`)
4. **An adapter that answers a switch with an empty or unparseable catalogue, answers late, or never answers** (it handles requests concurrently). Expected: the start or resume still succeeds, and the stored model and mode survive. A hung switch costs at most `CONFIG_TIMEOUT` and answers `config_failed` well before the collector's 60 s timeout would drop the host connection. A late model switch cannot clamp a mode switched after it: nothing more is sent at start, and live switches go out one at a time. (Task 4: `a_switch_without_a_read_back_announces_no_catalogue`, `a_hung_switch_is_reported_and_the_session_still_starts`, `a_late_model_switch_stops_the_starts_switches_so_its_clamp_cannot_undo_the_mode`; Task 5: `set_config_sends_one_switch_at_a_time_so_a_late_clamp_cannot_undo_a_later_switch`; Task 5: `a_switch_that_never_answers_is_config_failed_and_one_still_out_at_the_end_is_not_attached`; Task 6: `an_empty_or_absent_read_back_keeps_the_stored_catalogue`)
5. **A resume onto an adapter that no longer offers a stored value** (a pin bump dropped a model). Expected: the resume succeeds with a `host_note{reapply_failed}`, and the session takes prompts. (Task 4: `a_resume_re_applies_the_stored_config_and_a_failed_re_apply_is_only_a_note`)

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/frames.rs` | `SessionBody::session_started` (1); `ConfigValue`, `SessionConfig`, the catalogue extracts and `Indexed::current_config`, `session_started.indexed`, `ConfigApplied`, `config` on start/resume, `SetConfig` (2) | 1, 2 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs` | `StartSessionRequest.config`, `ConfigRequest`, `SessionCatalog` | 2 |
| `crates/hennery-testkit/src/lib.rs`, `src/bin/hennery-fake-acp.rs` | Fake config options and switches, `sample_config_options` (3); `flood` (10) | 3, 10 |
| `crates/hennery-host/src/session.rs` | `Launch`, `launch`, `CONFIG_TIMEOUT`, `apply_config`, `catalogue_extracts` (4); `SessionCmd::SetConfig`, `PendingConfigs`, `live_update` (5); `begin_ending` (9); `UPDATE_BURST` (10) | 4, 5, 9, 10 |
| `crates/hennery-host/src/connection.rs` | `AttachRequest.config` through `session::launch` (4); `set_config` dispatch (5) | 2, 4, 5, 9 |
| `crates/hennery-sessions/src/store.rs` | Migration 5, `store_catalogue`, `SessionRow.config`, `ResumeRequest::Starting.config`, `Store::catalog` | 2, 6 |
| `crates/hennery-sessions/src/api.rs`, `ws.rs` | Start and resume carry the config (2, 6); `POST …/config`, `GET …/catalog`, `catalog_changed`, `config_applied` resolves its waiter (7) | 2, 6, 7 |
| `crates/hennery-proto/tests/{frames,codegen}.rs`, `crates/hennery-testkit/tests/{fake_acp,host_session,host_connection,reconcile,e2e,ws_ingest_error}.rs`, `crates/hennery-sessions/tests/store.rs` | Tests | all |

All commands run from the repository root inside the dev shell (`nix develop`, or direnv). Work on a feature branch off `main` (e.g. `feat/session-config`). Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "Replace the whole of `path` with:" overwrites the file.
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point (earlier blocks of the same task already applied, in order), then "with:" and its replacement.
- "Run this rewrite:" is followed by a shell block that edits files mechanically; run it from the repository root.

Other "Run:" lines only check; they change nothing. The plan was replayed exactly this way, from its own text, onto `75b2fc0`.

---

### Task 1: One constructor for `session_started`, and patterns that survive new fields

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`
- Test: `crates/hennery-proto/tests/frames.rs`
- Modify (tests only): `crates/hennery-sessions/tests/store.rs`, `crates/hennery-testkit/tests/{reconcile,ws_ingest_error,host_connection,host_session}.rs`

**Interfaces:**
- Produces: `SessionBody::session_started(request_id: impl Into<String>, agent_session_id: impl Into<String>) -> SessionBody`. Task 2 adds `indexed: Indexed::default()` inside it, so no test literal has to change when `session_started` gains a field.
- Produces: every destructuring pattern of `SessionStarted`, `ResumeSession` and `ResumeRequest::Starting` in the tests ends in `..`.

This task changes no behaviour. It exists so that Task 2's new fields are one-line changes (B2a's churn warning).

- [ ] **Step 1: Write the failing test**

Append to `crates/hennery-proto/tests/frames.rs`:

```rust
#[test]
fn session_started_names_the_request_and_the_agents_session() {
    let body = serde_json::to_value(SessionBody::session_started("r", "a")).unwrap();
    assert_eq!(
        (&body["kind"], &body["request_id"], &body["agent_session_id"]),
        (&json!("session_started"), &json!("r"), &json!("a"))
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p hennery-proto --test frames`
Expected: does not compile: `no variant, associated function, or constant named 'session_started' found for enum 'SessionBody'`.

- [ ] **Step 3: Implement the constructor**

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
}

/// Host -> collector.
```

with:

```rust
}

impl SessionBody {
    /// A `session_started` with nothing but its ids, for tests and for
    /// callers that have no catalogue to announce.
    pub fn session_started(request_id: impl Into<String>, agent_session_id: impl Into<String>) -> Self {
        Self::SessionStarted {
            request_id: request_id.into(),
            agent_session_id: agent_session_id.into(),
        }
    }
}

/// Host -> collector.
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p hennery-proto --test frames`
Expected: all 11 pass.

- [ ] **Step 5: Build every `session_started` literal in the tests through the constructor**

Run this rewrite:

```bash
perl -0pi -e 's/SessionBody::SessionStarted \{\s*request_id(?:: ("[^"]*")\.into\(\)|: (\w+))?,\s*agent_session_id: ("[^"]*")\.into\(\),\s*\}/"SessionBody::session_started(".($1 \/\/ $2 \/\/ "request_id").", $3)"/ge' \
  crates/hennery-sessions/tests/store.rs crates/hennery-testkit/tests/reconcile.rs crates/hennery-testkit/tests/ws_ingest_error.rs
cargo fmt --all
```

Run: `grep -rn 'SessionBody::SessionStarted {' crates --include='*.rs'`
Expected: only the enum and the constructor in `frames.rs`, the host actor's two emits in `session.rs`, and patterns (`store.rs`, `ws.rs`, `host_session.rs`, `host_connection.rs`). No test literal is left.

- [ ] **Step 6: End the remaining full-field patterns with `..`**

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
                    request_id,
                    agent_session_id,
                },
```

with:

```rust
                    request_id,
                    agent_session_id,
                    ..
                },
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
                        request_id,
                        agent_session_id,
                    },
```

with:

```rust
                        request_id,
                        agent_session_id,
                        ..
                    },
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            cwd,
            agent_session_id,
        } => {
```

with:

```rust
            cwd,
            agent_session_id,
            ..
        } => {
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
        agent_session_id,
        committed_seq,
    } = store.request_resume("s1").unwrap()
```

with:

```rust
        agent_session_id,
        committed_seq,
        ..
    } = store.request_resume("s1").unwrap()
```

- [ ] **Step 7: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates
git commit -m "refactor(proto): build session_started through one constructor"
```

Expected: 196 tests pass.

---

### Task 2: Wire types for model, axes and mode

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`
- Test: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-proto/tests/codegen.rs`
- Modify (to keep the workspace compiling): `crates/hennery-host/src/session.rs`, `crates/hennery-host/src/connection.rs`, `crates/hennery-sessions/src/store.rs`, `crates/hennery-sessions/src/api.rs`, `crates/hennery-testkit/tests/{host_session,host_connection,reconcile}.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`

**Interfaces:**
- Produces (`hennery_proto::frames`):
  - `enum ConfigValue { Bool(bool), Id(String) }` (untagged);
  - `struct SessionConfig { model: Option<String>, mode: Option<String>, axes: BTreeMap<String, ConfigValue> }` (`Default`), with `fn is_empty(&self) -> bool`;
  - `Indexed` gains `config_options: Option<Vec<Value>>`, `current_model: Option<String>`, `current_mode: Option<String>`, `current_axes: Option<BTreeMap<String, ConfigValue>>`, and `fn current_config(&self) -> Option<SessionConfig>` (`None` unless `config_options` is present and non-empty);
  - `SessionBody::SessionStarted { request_id, agent_session_id, indexed: Indexed }` and `SessionBody::ConfigApplied { request_id: String, indexed: Indexed }`;
  - `CollectorFrame::StartSession { …, config: SessionConfig }`, `CollectorFrame::ResumeSession { …, config: SessionConfig }` (both `#[serde(flatten)]`);
  - `CollectorFrame::SetConfig { request_id: String, session_id: String, config_id: String, value: ConfigValue }`.
- Produces (`hennery_proto::rest`): `StartSessionRequest.config: SessionConfig` (flattened), `ConfigRequest { config_id: String, value: ConfigValue }`, `SessionCatalog { session_id: String, config_options: Vec<Value>, current: SessionConfig }` (`current` flattened), with `SessionCatalog::from_indexed(session_id: &str, indexed: &Indexed) -> Option<SessionCatalog>`.
- Interim behaviour, until the named task replaces it:
  - the host ignores the config (Task 4), and answers `set_config` with `error{invalid}` (Task 5);
  - the collector stores `config_applied` as a fact without a transition (Task 6);
  - the collector sends an empty config on resume (Task 6).
  - The start request's config already reaches the frame.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
use hennery_proto::frames::{CollectorFrame, HostFrame, Indexed, ParkReason, SessionBody, TurnOutcome};
use serde_json::json;
```

with:

```rust
use hennery_proto::frames::{
    CollectorFrame, ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome,
};
use serde_json::json;
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            agent: "claude".into(),
            cwd: "/tmp".into(),
        },
```

with:

```rust
            agent: "claude".into(),
            cwd: "/tmp".into(),
            config: Default::default(),
        },
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            cwd: "/tmp".into(),
            agent_session_id: "a1".into(),
        },
```

with:

```rust
            cwd: "/tmp".into(),
            agent_session_id: "a1".into(),
            config: Default::default(),
        },
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            session_id: "s".into(),
            turn_id: "t".into(),
        },
```

with:

```rust
            session_id: "s".into(),
            turn_id: "t".into(),
        },
        CollectorFrame::SetConfig {
            request_id: "r".into(),
            session_id: "s".into(),
            config_id: "model".into(),
            value: ConfigValue::Id("large".into()),
        },
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        cwd: "/tmp".into(),
        agent_session_id: "a1".into(),
    };
```

with:

```rust
        cwd: "/tmp".into(),
        agent_session_id: "a1".into(),
        config: Default::default(),
    };
```

Append to `crates/hennery-proto/tests/frames.rs`:

```rust
// Plan B2b: model, axes and mode (ACP core §3.2, §3.3).

fn config() -> SessionConfig {
    SessionConfig {
        model: Some("large".into()),
        mode: Some("plan".into()),
        axes: [
            ("effort".to_string(), ConfigValue::Id("high".into())),
            ("fast".to_string(), ConfigValue::Bool(true)),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn start_and_resume_carry_model_mode_and_axes_as_flat_fields() {
    let start = CollectorFrame::StartSession {
        request_id: "r".into(),
        session_id: "s".into(),
        committed_seq: 0,
        agent: "claude".into(),
        cwd: "/tmp".into(),
        config: config(),
    };
    let expected = json!({
        "type": "start_session", "request_id": "r", "session_id": "s", "committed_seq": 0,
        "agent": "claude", "cwd": "/tmp",
        "model": "large", "mode": "plan", "axes": {"effort": "high", "fast": true}
    });
    assert_eq!(serde_json::to_value(&start).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), start);
    // Absent fields are an empty config, for a collector that sends none.
    let bare: CollectorFrame = serde_json::from_value(json!({
        "type": "resume_session", "request_id": "r", "session_id": "s", "committed_seq": 3,
        "agent": "claude", "cwd": "/tmp", "agent_session_id": "a1"
    }))
    .unwrap();
    let CollectorFrame::ResumeSession { config, .. } = bare else {
        panic!("{bare:?}");
    };
    assert!(config.is_empty());
}

#[test]
fn config_frames_use_the_spec_field_names() {
    let set = CollectorFrame::SetConfig {
        request_id: "r".into(),
        session_id: "s".into(),
        config_id: "fast".into(),
        value: ConfigValue::Bool(false),
    };
    assert_eq!(
        serde_json::to_value(&set).unwrap(),
        json!({"type": "set_config", "request_id": "r", "session_id": "s", "config_id": "fast", "value": false})
    );
    let applied = SessionBody::ConfigApplied {
        request_id: "r".into(),
        indexed: Indexed {
            config_options: Some(vec![json!({"id": "model"})]),
            current_model: Some("large".into()),
            current_mode: Some("plan".into()),
            current_axes: Some(config().axes),
            ..Indexed::default()
        },
    };
    let expected = json!({
        "kind": "config_applied", "request_id": "r",
        "indexed": {
            "config_options": [{"id": "model"}], "current_model": "large", "current_mode": "plan",
            "current_axes": {"effort": "high", "fast": true}
        }
    });
    assert_eq!(serde_json::to_value(&applied).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), applied);
    // A `session_started` from an older host has no extracts.
    let old: SessionBody =
        serde_json::from_value(json!({"kind": "session_started", "request_id": "r", "agent_session_id": "a"})).unwrap();
    assert_eq!(old, SessionBody::session_started("r", "a"));
}

#[test]
fn only_a_non_empty_catalogue_is_a_snapshot_of_the_current_config() {
    let mut indexed = Indexed {
        current_model: Some("large".into()),
        ..Indexed::default()
    };
    assert_eq!(indexed.current_config(), None, "no catalogue");
    indexed.config_options = Some(vec![]);
    assert_eq!(indexed.current_config(), None, "an empty read-back");
    indexed.config_options = Some(vec![json!({"id": "model"})]);
    assert_eq!(
        indexed.current_config(),
        Some(SessionConfig {
            model: Some("large".into()),
            ..SessionConfig::default()
        })
    );
}

#[test]
fn rest_config_requests_take_a_value_id_or_a_boolean() {
    use hennery_proto::rest::{ConfigRequest, StartSessionRequest};
    let start: StartSessionRequest =
        serde_json::from_value(json!({"host_id": "h", "agent": "claude", "cwd": "/tmp", "mode": "plan"})).unwrap();
    assert_eq!(start.config.mode.as_deref(), Some("plan"));
    let plain: StartSessionRequest =
        serde_json::from_value(json!({"host_id": "h", "agent": "claude", "cwd": "/tmp"})).unwrap();
    assert!(plain.config.is_empty());
    let id: ConfigRequest = serde_json::from_value(json!({"config_id": "model", "value": "large"})).unwrap();
    assert_eq!(id.value, ConfigValue::Id("large".into()));
    let toggle: ConfigRequest = serde_json::from_value(json!({"config_id": "fast", "value": true})).unwrap();
    assert_eq!(toggle.value, ConfigValue::Bool(true));
    assert!(serde_json::from_value::<ConfigRequest>(json!({"config_id": "fast", "value": 3})).is_err());
}
```

Append to `crates/hennery-proto/tests/codegen.rs`:

```rust
#[test]
fn config_fields_are_flat_in_typescript() {
    let ts = render_ts();
    assert!(ts.contains("export type ConfigValue = boolean | string;"), "{ts}");
    let start = ts
        .lines()
        .find(|l| l.starts_with("export type StartSessionRequest ="))
        .expect("StartSessionRequest");
    assert!(start.contains("model?") && start.contains("axes?"), "{start}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-proto`
Expected: does not compile: `ConfigValue` and `SessionConfig` are not in `hennery_proto::frames`, and `CollectorFrame` has no variant `SetConfig`.

- [ ] **Step 3: Implement the wire types**

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
}

/// Fields the collector may read from a session event. Closed set (ACP core §3.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
```

with:

```rust
}

/// The value of one config option (ACP `session/set_config_option`): a
/// select's value id, or a boolean toggle's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum ConfigValue {
    Bool(bool),
    Id(String),
}

/// Model, mode and the other config axes of a session (ACP core §3.3,
/// §4.3): what a start asks for, and what a resume re-applies. `model` and
/// `mode` are the values of the adapter's model and mode options; `axes`
/// holds every other option, by config id. Flattened into the frames and
/// the start request, so the wire stays `model?, mode?, axes{}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub axes: BTreeMap<String, ConfigValue>,
}

impl SessionConfig {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.mode.is_none() && self.axes.is_empty()
    }
}

/// Fields the collector may read from a session event. Closed set (ACP core §3.2).
///
/// The catalogue extracts (`config_options`, `current_model`,
/// `current_mode`, `current_axes`) travel together: when `config_options`
/// is present and not empty, the four are one snapshot of the adapter's
/// config. An absent or empty `config_options` means "no read-back", never
/// "the adapter has no config" (a response that failed to parse looks
/// empty).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
```

with:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The full config catalogue: the adapter's ACP `SessionConfigOption`
    /// objects, for the UI. The collector stores it and never reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "unknown[] | undefined", optional)]
    pub config_options: Option<Vec<Value>>,
    /// The current value of the model option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_model: Option<String>,
    /// The current value of the mode option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<String>,
    /// The current value of every other option, by config id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_axes: Option<BTreeMap<String, ConfigValue>>,
}

impl Indexed {
    /// The config this event reports as current, if it carries a catalogue
    /// snapshot (see the type's doc).
    pub fn current_config(&self) -> Option<SessionConfig> {
        let options = self.config_options.as_ref()?;
        if options.is_empty() {
            return None;
        }
        Some(SessionConfig {
            model: self.current_model.clone(),
            mode: self.current_mode.clone(),
            axes: self.current_axes.clone().unwrap_or_default(),
        })
    }
}
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
pub enum SessionBody {
    /// The adapter session exists. Resolves the collector's start waiter.
    SessionStarted {
        request_id: String,
        agent_session_id: String,
    },
```

with:

```rust
pub enum SessionBody {
    /// The adapter session exists. Resolves the collector's start waiter.
    /// `indexed` carries the catalogue after the start's config switches
    /// (ACP core §4.3, P-13).
    SessionStarted {
        request_id: String,
        agent_session_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    /// `note` is a machine code; `text` is scrubbed.
    HostNote { note: String, text: String },
}
```

with:

```rust
    /// `note` is a machine code; `text` is scrubbed.
    HostNote { note: String, text: String },
    /// A `set_config` took effect: the catalogue the adapter answered with,
    /// the authoritative read-back (ACP core §3.2). Resolves the collector's
    /// config waiter.
    ConfigApplied {
        request_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
}
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
            request_id: request_id.into(),
            agent_session_id: agent_session_id.into(),
        }
```

with:

```rust
            request_id: request_id.into(),
            agent_session_id: agent_session_id.into(),
            indexed: Indexed::default(),
        }
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        agent: String,
        cwd: String,
    },
```

with:

```rust
        agent: String,
        cwd: String,
        /// Applied after `session/new`: model, then the other axes, then
        /// mode (ACP core §4.3).
        #[serde(flatten)]
        config: SessionConfig,
    },
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        /// the host keeps no copy across restarts.
        agent_session_id: String,
    },
```

with:

```rust
        /// the host keeps no copy across restarts.
        agent_session_id: String,
        /// The stored config, re-applied after `session/load` (ACP core
        /// §4.3). A switch that fails is a `host_note`, not a failed resume.
        #[serde(flatten)]
        config: SessionConfig,
    },
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        turn_id: String,
    },
    Ack {
```

with:

```rust
        turn_id: String,
    },
    /// Switch one config option of an attached session (`session/set_config_option`).
    /// Completed by `config_applied` | `error` (ACP core §3.3).
    SetConfig {
        request_id: String,
        session_id: String,
        config_id: String,
        value: ConfigValue,
    },
    Ack {
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
```

with:

```rust
use ts_rs::TS;

use crate::frames::{ConfigValue, Indexed, SessionConfig};

/// `POST /api/sessions` (ACP core §9): `{host_id, agent, cwd, model?, mode?, axes?}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    pub agent: String,
    pub cwd: String,
}
```

with:

```rust
    pub agent: String,
    pub cwd: String,
    #[serde(flatten)]
    pub config: SessionConfig,
}
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// `POST /api/sessions/{id}/config`: switch one config option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ConfigRequest {
    pub config_id: String,
    pub value: ConfigValue,
}

/// A session's config catalogue and its current values: `GET
/// /api/sessions/{id}/catalog`, the answer to `POST …/config`, and the data
/// of the SSE `catalog_changed` message (ACP core §9). Commands, plan and
/// usage join it with the plans that produce them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionCatalog {
    pub session_id: String,
    /// The adapter's ACP `SessionConfigOption` objects, as last reported.
    #[ts(type = "unknown[]")]
    pub config_options: Vec<Value>,
    #[serde(flatten)]
    pub current: SessionConfig,
}

impl SessionCatalog {
    /// The catalogue an event's extracts report, if they carry a snapshot.
    pub fn from_indexed(session_id: &str, indexed: &Indexed) -> Option<Self> {
        let current = indexed.current_config()?;
        Some(Self {
            session_id: session_id.to_string(),
            config_options: indexed.config_options.clone().unwrap_or_default(),
            current,
        })
    }
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SessionDetail,
        rest::CancelResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::SessionDetail,
        rest::CancelResponse,
        rest::ConfigRequest,
        rest::SessionCatalog,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
    }
    add!(
        frames::Capability,
```

with:

```rust
    }
    add!(
        frames::ConfigValue,
        frames::SessionConfig,
        frames::Capability,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SessionDetail,
        rest::CancelResponse,
    );
```

with:

```rust
        rest::SessionDetail,
        rest::CancelResponse,
        rest::ConfigRequest,
        rest::SessionCatalog,
    );
```

- [ ] **Step 4: Keep the workspace compiling**

The host emits `session_started` with no extracts yet, ignores the config, and refuses `set_config`:

In `crates/hennery-host/src/session.rs`, replace:

```rust
            request_id,
            agent_session_id: agent_session.to_string(),
        });
```

with:

```rust
            request_id,
            agent_session_id: agent_session.to_string(),
            indexed: Indexed::default(),
        });
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                        request_id,
                        agent_session_id: agent_session.to_string(),
                    }),
```

with:

```rust
                        request_id,
                        agent_session_id: agent_session.to_string(),
                        indexed: Indexed::default(),
                    }),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            agent,
            cwd,
        } => attach(
```

with:

```rust
            agent,
            cwd,
            ..
        } => attach(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            cwd,
            agent_session_id,
        } => attach(
```

with:

```rust
            cwd,
            agent_session_id,
            ..
        } => attach(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

with:

```rust
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::SetConfig { request_id, .. } => uplink.reply(HostFrame::Error {
            request_id,
            code: "invalid".into(),
            message: "this host does not support set_config yet".into(),
        }),
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

The collector keeps `config_applied` as a plain fact, forwards the start request's config, and sends no config on resume yet:

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } => {
                if !fact_applies(&tx, session_id, None)? {
```

with:

```rust
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing; nor does a `config_applied` until the
            // catalogue is stored.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } | SessionBody::ConfigApplied { .. } => {
                if !fact_applies(&tx, session_id, None)? {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        SessionBody::AdapterExited { .. } => "adapter_exited",
        SessionBody::HostNote { .. } => "host_note",
    }
```

with:

```rust
        SessionBody::AdapterExited { .. } => "adapter_exited",
        SessionBody::HostNote { .. } => "host_note",
        SessionBody::ConfigApplied { .. } => "config_applied",
    }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, SessionBody};
use hennery_proto::rest::{
```

with:

```rust
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, SessionBody, SessionConfig};
use hennery_proto::rest::{
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        agent: req.agent,
        cwd: req.cwd,
    };
```

with:

```rust
        agent: req.agent,
        cwd: req.cwd,
        config: req.config,
    };
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        cwd: session.cwd,
        agent_session_id,
    };
```

with:

```rust
        cwd: session.cwd,
        agent_session_id,
        config: SessionConfig::default(),
    };
```

The test literals and the one exhaustive match in the tests:

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
                SessionBody::AdapterExited { .. } => "adapter_exited".to_string(),
                SessionBody::HostNote { note, .. } => format!("host_note:{note}"),
            },
```

with:

```rust
                SessionBody::AdapterExited { .. } => "adapter_exited".to_string(),
                SessionBody::HostNote { note, .. } => format!("host_note:{note}"),
                SessionBody::ConfigApplied { .. } => "config_applied".to_string(),
            },
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        agent: "fake".into(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
    }
```

with:

```rust
        agent: "fake".into(),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        config: Default::default(),
    }
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        agent_session_id: agent_session_id.into(),
    }
```

with:

```rust
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        agent_session_id: agent_session_id.into(),
        config: Default::default(),
    }
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        agent: "fake".into(),
        cwd: "/tmp".into(),
    };
```

with:

```rust
        agent: "fake".into(),
        cwd: "/tmp".into(),
        config: Default::default(),
    };
```

- [ ] **Step 5: Regenerate and run the tests**

Run: `cargo run -p hennery-proto --bin gen`
Expected: `wrote schema/hennery-protocol.schema.json`, `wrote web/src/generated/protocol.ts`.

Run: `cargo test -p hennery-proto`
Expected: `frames` 15 pass, `codegen` 6 pass.

- [ ] **Step 6: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
cargo run -p hennery-proto --bin gen -- --check
git add crates schema web
git commit -m "feat(proto): model, mode and axes on start and resume; set_config and config_applied"
```

Expected: 201 tests pass.

---

### Task 3: Fake adapter: config options

**Files:**
- Modify: `crates/hennery-testkit/src/lib.rs`
- Replace: `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`
- Test: `crates/hennery-testkit/tests/fake_acp.rs`

**Interfaces:**
- Consumes: agent-client-protocol-schema 1.9.1 `SessionConfigOption`, `SetSessionConfigOptionRequest` / `Response`, `ConfigOptionUpdate`.
- Produces: new `FakeScript` fields, all `#[serde(default)]`:
  - `config_options: Vec<Value>`: ACP option JSON, announced by `session/new` and `session/load` (absent when empty), and switched by `session/set_config_option`. An unknown id, or a value the option does not offer, is refused with `-32602`, like a real adapter. A boolean option is announced as a boolean only if `initialize` advertised `session.configOptions.boolean`; otherwise it becomes an `on` / `off` select;
  - `model_switch_sets_mode: Option<String>`: a model switch also sets the mode option to this value (§12 scenario 1's clamp);
  - `prompt_sets_mode: Option<String>`: every prompt first sets the mode and sends a `config_option_update`;
  - `config_log: Option<String>`: a file that gets one `id=value` line per call, refused calls included;
  - `empty_config_read_back: bool`: apply, but answer `configOptions: []`;
  - `hang_config: bool`: never answer a switch;
  - `slow_model_switch_ms: Option<u64>`: answer a model switch this late, from a task of its own so other requests are handled meanwhile (like the adapters' TS SDK), and apply `model_switch_sets_mode` only after that answer;
  - `sticky_options: Vec<String>`: switches of these ids are accepted and change nothing;
  - `config_in_update_only: bool`: announce the options in a `config_option_update` sent just before the `session/new` / `session/load` answer, which has none.
- Produces: `hennery_testkit::sample_config_options() -> Vec<Value>`:
  - `model` (category `model`: `small` | `large`, current `small`);
  - `effort` (category `thought_level`: `low` | `high`, current `low`);
  - `fast` (a boolean, off);
  - `mode` (category `mode`: `default` | `plan` | `bypass`, current `default`).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/fake_acp.rs`:

```rust
// Plan B2b: config options.

fn config_script(extra: Value) -> String {
    let mut script = json!({ "chunks": [], "config_options": hennery_testkit::sample_config_options() });
    script
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    script.to_string()
}

/// `session/set_config_option`; a boolean value carries ACP's `type` tag.
fn set_config(id: i64, config_id: &str, value: Value) -> Value {
    let mut params = json!({"sessionId": "fake-session-1", "configId": config_id, "value": value});
    if value.is_boolean() {
        params["type"] = json!("boolean");
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "session/set_config_option", "params": params})
}

/// `requests` with an `initialize` that advertises boolean config options,
/// as the hennery host does.
fn with_booleans(mut requests: Vec<Value>) -> Vec<Value> {
    requests[0]["params"]["clientCapabilities"] = json!({"session": {"configOptions": {"boolean": {}}}});
    requests
}

/// The current value of every option in a `configOptions` list.
fn current(options: &Value) -> Vec<(String, Value)> {
    options
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["id"].as_str().unwrap().to_string(), o["currentValue"].clone()))
        .collect()
}

#[test]
fn session_new_and_load_announce_the_scripted_config_options() {
    let script = config_script(json!({}));
    let out = exchange_until(&script, &with_booleans(session_requests()[..2].to_vec()), 2);
    let options = &out.last().unwrap()["result"]["configOptions"];
    assert_eq!(
        current(options),
        [
            ("model".to_string(), json!("small")),
            ("effort".to_string(), json!("low")),
            ("fast".to_string(), json!(false)),
            ("mode".to_string(), json!("default"))
        ]
    );
    let out = exchange_until(&script, &with_booleans(load_requests("agent-7")), 2);
    assert_eq!(out.last().unwrap()["result"]["configOptions"], *options);
    // A client that cannot show a boolean option gets an on/off select.
    let out = exchange_until(&script, &session_requests()[..2], 2);
    let fast = &out.last().unwrap()["result"]["configOptions"][2];
    assert_eq!(
        (&fast["type"], &fast["currentValue"]),
        (&json!("select"), &json!("off")),
        "{fast}"
    );
    // No scripted options: none announced, like an adapter without them.
    let out = exchange_until(r#"{"chunks":[]}"#, &session_requests()[..2], 2);
    assert!(out.last().unwrap()["result"].get("configOptions").is_none(), "{out:?}");
}

#[test]
fn set_config_option_switches_validates_and_clamps_the_mode_like_an_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let script = config_script(json!({ "model_switch_sets_mode": "default", "config_log": log }));
    let mut requests = with_booleans(session_requests()[..2].to_vec());
    requests.push(set_config(3, "mode", json!("plan")));
    requests.push(set_config(4, "model", json!("large")));
    requests.push(set_config(5, "fast", json!(true)));
    requests.push(set_config(6, "model", json!("huge")));
    requests.push(set_config(7, "nope", json!("x")));
    let out = exchange_until(&script, &requests, 7);
    let answer = |id: i64| out.iter().find(|m| m["id"] == json!(id)).unwrap();
    assert_eq!(current(&answer(3)["result"]["configOptions"])[3].1, json!("plan"));
    // The model switch resets the mode.
    assert_eq!(
        current(&answer(5)["result"]["configOptions"]),
        [
            ("model".to_string(), json!("large")),
            ("effort".to_string(), json!("low")),
            ("fast".to_string(), json!(true)),
            ("mode".to_string(), json!("default"))
        ]
    );
    assert_eq!(answer(6)["error"]["code"], -32602, "a value the option does not offer");
    assert_eq!(answer(7)["error"]["code"], -32602, "an unknown option");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "mode=plan\nmodel=large\nfast=true\nmodel=huge\nnope=x\n"
    );
}

#[test]
fn an_empty_read_back_still_applies_the_switch() {
    let script = config_script(json!({ "empty_config_read_back": true, "prompt_sets_mode": "bypass" }));
    let mut requests = with_booleans(session_requests()[..2].to_vec());
    requests.push(set_config(3, "model", json!("large")));
    requests.push(json!({"jsonrpc":"2.0","id":4,"method":"session/prompt",
                         "params":{"sessionId":"fake-session-1","prompt":[{"type":"text","text":"hi"}]}}));
    let out = exchange_until(&script, &requests, 4);
    let answer = out.iter().find(|m| m["id"] == json!(3)).unwrap();
    assert_eq!(answer["result"]["configOptions"], json!([]), "{answer}");
    // The prompt's own mode change shows the switch took effect.
    let update = out
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "config_option_update")
        .expect("a config_option_update");
    assert_eq!(
        current(&update["params"]["update"]["configOptions"]),
        [
            ("model".to_string(), json!("large")),
            ("effort".to_string(), json!("low")),
            ("fast".to_string(), json!(false)),
            ("mode".to_string(), json!("bypass"))
        ]
    );
}

#[test]
fn a_sticky_option_accepts_a_switch_and_keeps_its_value() {
    let script = config_script(json!({ "sticky_options": ["effort"] }));
    let mut requests = session_requests()[..2].to_vec();
    requests.push(set_config(3, "effort", json!("high")));
    let out = exchange_until(&script, &requests, 3);
    let answer = out.last().unwrap();
    assert_eq!(
        current(&answer["result"]["configOptions"])[1].1,
        json!("low"),
        "{answer}"
    );
}

#[test]
fn config_in_update_only_announces_the_options_before_the_answer() {
    let script = config_script(json!({ "config_in_update_only": true }));
    let out = exchange_until(&script, &session_requests()[..2], 2);
    let answer = out.last().unwrap();
    assert!(answer["result"].get("configOptions").is_none(), "{answer}");
    let update = out
        .iter()
        .find(|m| m["params"]["update"]["sessionUpdate"] == "config_option_update")
        .expect("a config_option_update before the answer");
    assert_eq!(
        current(&update["params"]["update"]["configOptions"])[0].1,
        json!("small")
    );
}

/// The real adapters' SDK handles requests concurrently: a slow model
/// switch does not hold back the mode switch sent after it, and its mode
/// clamp lands after its own answer.
#[test]
fn a_slow_model_switch_is_answered_after_a_later_switch_and_clamps_after_answering() {
    let script = config_script(json!({ "slow_model_switch_ms": 300, "model_switch_sets_mode": "default" }));
    let mut requests = session_requests()[..2].to_vec();
    requests.push(set_config(3, "model", json!("large")));
    requests.push(set_config(4, "mode", json!("plan")));
    let out = exchange_until(&script, &requests, 3);
    let order: Vec<i64> = out
        .iter()
        .filter_map(|m| m["id"].as_i64())
        .filter(|id| *id >= 3)
        .collect();
    assert_eq!(order, [4, 3], "{out:?}");
    // Its answer still shows the mode the later switch set: the clamp came after.
    let model = out.last().unwrap();
    assert_eq!(
        (
            current(&model["result"]["configOptions"])[0].1.clone(),
            current(&model["result"]["configOptions"])[3].1.clone()
        ),
        (json!("large"), json!("plan"))
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test fake_acp`
Expected: does not compile: `cannot find function 'sample_config_options' in crate 'hennery_testkit'`.

- [ ] **Step 3: Implement**

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_error: Option<i32>,
}
```

with:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_error: Option<i32>,
    /// Config options (ACP `SessionConfigOption` JSON) announced by
    /// `session/new` and `session/load` and switched by
    /// `session/set_config_option`. A switch is validated like a real
    /// adapter does: an unknown id, or a value the option does not offer, is
    /// refused with `-32602`. A boolean option is announced as a boolean
    /// only to a client whose `initialize` advertises
    /// `session.configOptions.boolean`; any other client gets an `on` /
    /// `off` select instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_options: Vec<serde_json::Value>,
    /// A switch of the model option also sets the mode option to this value
    /// (a model that clamps the mode, ACP core §12 scenario 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_sets_mode: Option<String>,
    /// Every prompt first sets the mode option to this value and announces
    /// it with a `config_option_update` (an agent that leaves plan mode on
    /// its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sets_mode: Option<String>,
    /// Append one `id=value` line per `session/set_config_option` call to
    /// this file, refused calls included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_log: Option<String>,
    /// Apply switches but answer them with an empty `configOptions` list (a
    /// read-back the host cannot use).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub empty_config_read_back: bool,
    /// Never answer `session/set_config_option` (a hung switch).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hang_config: bool,
    /// Answer a switch of the model option only after this many
    /// milliseconds, while other requests are handled meanwhile, and apply
    /// `model_switch_sets_mode` only after that answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_model_switch_ms: Option<u64>,
    /// Switches of these option ids are accepted but change nothing (an
    /// adapter that reports a value other than the one it was given).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sticky_options: Vec<String>,
    /// Announce the config options in a `config_option_update` sent just
    /// before the `session/new` / `session/load` answer, which then has
    /// none.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub config_in_update_only: bool,
}
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            ignore_cancel: false,
            cancel_error: None,
        }
```

with:

```rust
            ignore_cancel: false,
            cancel_error: None,
            config_options: Vec::new(),
            model_switch_sets_mode: None,
            prompt_sets_mode: None,
            config_log: None,
            empty_config_read_back: false,
            hang_config: false,
            slow_model_switch_ms: None,
            sticky_options: Vec::new(),
            config_in_update_only: false,
        }
```

Append to `crates/hennery-testkit/src/lib.rs`:

```rust
/// A config catalogue like a real adapter's, for `FakeScript::config_options`:
/// `model` (category `model`: `small` | `large`, current `small`), `effort`
/// (category `thought_level`: `low` | `high`, current `low`), `fast` (a
/// boolean, off) and `mode` (category `mode`: `default` | `plan` |
/// `bypass`, current `default`).
pub fn sample_config_options() -> Vec<serde_json::Value> {
    let select = |id: &str, category: &str, current: &str, values: &[&str]| {
        let options: Vec<serde_json::Value> = values
            .iter()
            .map(|v| serde_json::json!({ "value": v, "name": v }))
            .collect();
        serde_json::json!({
            "id": id, "name": id, "category": category, "type": "select",
            "currentValue": current, "options": options
        })
    };
    vec![
        select("model", "model", "small", &["small", "large"]),
        select("effort", "thought_level", "low", &["low", "high"]),
        serde_json::json!({ "id": "fast", "name": "fast", "type": "boolean", "currentValue": false }),
        select("mode", "mode", "default", &["default", "plan", "bypass"]),
    ]
}
```

Replace the whole of `crates/hennery-testkit/src/bin/hennery-fake-acp.rs` with:

```rust
//! A scripted ACP agent for tests. Speaks ACP over stdio via the
//! `agent-client-protocol` crate, so the host is tested against the same
//! wire format real adapters use.

use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ConfigOptionUpdate, ContentBlock, ContentChunk, InitializeRequest,
    InitializeResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionConfigSelect, SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
    TextContent,
};
use agent_client_protocol::{Agent, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeScript, SCRIPT_ENV};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// The fake's config options, as the switches so far have left them.
type Catalogue = Arc<Mutex<Vec<SessionConfigOption>>>;

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

    let load_session = !script.no_load_session;
    let catalogue: Catalogue = Arc::new(Mutex::new(
        serde_json::from_value(serde_json::Value::Array(script.config_options.clone()))
            .expect("valid config options in the fake script"),
    ));
    // Absent, not empty, when the script has none: like an adapter without
    // config options.
    let announced = {
        let catalogue = catalogue.clone();
        move || {
            let options = catalogue.lock().unwrap().clone();
            (!options.is_empty()).then_some(options)
        }
    };
    // `session/cancel` for the prompt in flight: set by the notification,
    // cleared when a prompt starts. Handlers run in arrival order, so a
    // cancel sent right after its prompt is never cleared by that prompt.
    let cancel = Arc::new(watch::channel(false).0);
    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            {
                let catalogue = catalogue.clone();
                async move |req: InitializeRequest, responder, _cx| {
                    // A boolean option is announced as one only to a client
                    // that says it can show one; any other gets an on/off
                    // select, as the real adapters do.
                    let booleans = req
                        .client_capabilities
                        .session
                        .as_ref()
                        .and_then(|session| session.config_options.as_ref())
                        .is_some_and(|options| options.boolean.is_some());
                    if !booleans {
                        booleans_as_selects(&mut catalogue.lock().unwrap());
                    }
                    responder.respond(
                        InitializeResponse::new(req.protocol_version)
                            .agent_capabilities(AgentCapabilities::new().load_session(load_session)),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let announced = announced.clone();
                async move |_req: NewSessionRequest, responder, cx| match script.new_session_error {
                    Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                    None if script.config_in_update_only => {
                        if let Some(options) = announced() {
                            cx.send_notification(SessionNotification::new(
                                "fake-session-1",
                                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                            ))?;
                        }
                        responder.respond(NewSessionResponse::new("fake-session-1"))
                    }
                    None => responder.respond(NewSessionResponse::new("fake-session-1").config_options(announced())),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let announced = announced.clone();
                async move |req: LoadSessionRequest, responder, cx| {
                    // History first, then the answer: an ACP agent replays a
                    // loaded session as `session/update`s before it responds.
                    for update in &script.replay {
                        cx.send_notification(UntypedMessage::new(
                            "session/update",
                            serde_json::json!({ "sessionId": req.session_id, "update": update }),
                        )?)?;
                    }
                    match script.load_error {
                        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
                        None if script.config_in_update_only => {
                            if let Some(options) = announced() {
                                cx.send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                                ))?;
                            }
                            responder.respond(LoadSessionResponse::new())
                        }
                        None => responder.respond(LoadSessionResponse::new().config_options(announced())),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let script = script.clone();
                let catalogue = catalogue.clone();
                async move |req: SetSessionConfigOptionRequest, responder, cx| {
                    log_switch(&script, &req);
                    if script.hang_config {
                        // Keep the responder alive, unanswered, for good.
                        return cx.spawn(async move {
                            std::future::pending::<()>().await;
                            drop(responder);
                            Ok(())
                        });
                    }
                    let is_model = catalogue
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|o| o.id == req.config_id && o.category == Some(SessionConfigOptionCategory::Model));
                    if let (Some(delay), true) = (script.slow_model_switch_ms, is_model) {
                        // Answered late, from a task of its own, so other
                        // requests are handled meanwhile (the TS SDK the real
                        // adapters use does not await one request before
                        // reading the next). The mode clamp lands only after
                        // the answer: the client cannot see it coming.
                        let script = script.clone();
                        let catalogue = catalogue.clone();
                        return cx.spawn(async move {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            let switched = {
                                let mut options = catalogue.lock().unwrap();
                                switch(&mut options, &req, None, &script.sticky_options).map(|()| options.clone())
                            };
                            let answered = match switched {
                                Ok(options) => responder.respond(SetSessionConfigOptionResponse::new(options)),
                                Err(message) => {
                                    responder.respond_with_error(agent_client_protocol::Error::new(-32602, message))
                                }
                            };
                            if let Some(mode) = &script.model_switch_sets_mode {
                                set_select(&mut catalogue.lock().unwrap(), &SessionConfigOptionCategory::Mode, mode);
                            }
                            answered
                        });
                    }
                    let switched = {
                        let mut options = catalogue.lock().unwrap();
                        switch(
                            &mut options,
                            &req,
                            script.model_switch_sets_mode.as_deref(),
                            &script.sticky_options,
                        )
                        .map(|()| options.clone())
                    };
                    match switched {
                        Ok(_) if script.empty_config_read_back => {
                            responder.respond(SetSessionConfigOptionResponse::new(Vec::new()))
                        }
                        Ok(options) => responder.respond(SetSessionConfigOptionResponse::new(options)),
                        Err(message) => {
                            responder.respond_with_error(agent_client_protocol::Error::new(-32602, message))
                        }
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
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
                    if let Some(mode) = &script.prompt_sets_mode {
                        let options = {
                            let mut options = catalogue.lock().unwrap();
                            set_select(&mut options, &SessionConfigOptionCategory::Mode, mode);
                            options.clone()
                        };
                        cx.send_notification(SessionNotification::new(
                            req.session_id.clone(),
                            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
                        ))?;
                    }
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

/// Record a switch in the script's `config_log`, if it has one.
fn log_switch(script: &FakeScript, req: &SetSessionConfigOptionRequest) {
    let Some(path) = &script.config_log else {
        return;
    };
    let value = match &req.value {
        SessionConfigOptionValue::ValueId { value } => value.to_string(),
        SessionConfigOptionValue::Boolean { value } => value.to_string(),
        other => format!("{other:?}"),
    };
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open the config log");
    writeln!(log, "{}={value}", req.config_id).expect("write the config log");
}

/// Apply one switch as a real adapter would, or say why it cannot. A switch
/// of a `sticky` option is accepted and changes nothing.
fn switch(
    options: &mut [SessionConfigOption],
    req: &SetSessionConfigOptionRequest,
    model_switch_sets_mode: Option<&str>,
    sticky: &[String],
) -> Result<(), String> {
    let option = options
        .iter_mut()
        .find(|o| o.id == req.config_id)
        .ok_or_else(|| format!("unknown config option {}", req.config_id))?;
    let is_model = option.category == Some(SessionConfigOptionCategory::Model);
    let sticky = sticky.iter().any(|id| **id == *req.config_id.0);
    match (&mut option.kind, &req.value) {
        (SessionConfigKind::Select(select), SessionConfigOptionValue::ValueId { value })
            if offers(&select.options, value) =>
        {
            if !sticky {
                select.current_value = value.clone();
            }
        }
        (SessionConfigKind::Boolean(toggle), SessionConfigOptionValue::Boolean { value }) => {
            if !sticky {
                toggle.current_value = *value;
            }
        }
        _ => return Err(format!("invalid value for {}", req.config_id)),
    }
    if is_model
        && !sticky
        && let Some(mode) = model_switch_sets_mode
    {
        set_select(options, &SessionConfigOptionCategory::Mode, mode);
    }
    Ok(())
}

fn offers(options: &SessionConfigSelectOptions, value: &SessionConfigValueId) -> bool {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options.iter().any(|o| &o.value == value),
        SessionConfigSelectOptions::Grouped(groups) => {
            groups.iter().flat_map(|g| &g.options).any(|o| &o.value == value)
        }
        _ => false,
    }
}

/// Turn every boolean option into an `on` / `off` select, for a client that
/// did not advertise boolean config options.
fn booleans_as_selects(options: &mut [SessionConfigOption]) {
    for option in options.iter_mut() {
        if let SessionConfigKind::Boolean(toggle) = &option.kind {
            let current = if toggle.current_value { "on" } else { "off" };
            let values = vec![
                SessionConfigSelectOption::new("on", "on"),
                SessionConfigSelectOption::new("off", "off"),
            ];
            option.kind = SessionConfigKind::Select(SessionConfigSelect::new(current, values));
        }
    }
}

/// Set the current value of the select option in `category`, unchecked.
fn set_select(options: &mut [SessionConfigOption], category: &SessionConfigOptionCategory, value: &str) {
    for option in options.iter_mut().filter(|o| o.category.as_ref() == Some(category)) {
        if let SessionConfigKind::Select(select) = &mut option.kind {
            select.current_value = SessionConfigValueId::new(value.to_string());
        }
    }
}

/// Exit mid-turn without answering the prompt. The short pause lets the
/// chunks already sent reach stdout first.
async fn crash() -> ! {
    tokio::time::sleep(Duration::from_millis(100)).await;
    eprintln!("fake-acp: crashing mid-turn");
    std::process::exit(CRASH_EXIT_CODE);
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test fake_acp`
Expected: all 15 pass.

- [ ] **Step 5: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-testkit
git commit -m "test(testkit): fake adapter config options and switches"
```

Expected: 207 tests pass.

---

### Task 4: Host: apply model, axes and mode on start and resume

**Files:**
- Modify: `crates/hennery-host/src/session.rs`, `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`, `crates/hennery-testkit/tests/host_connection.rs`, and a unit test at the end of `crates/hennery-host/src/session.rs` (the last block of Step 3)

**Interfaces:**
- Consumes: `SessionConfig`, `ConfigValue`, `Indexed`'s catalogue extracts (Task 2); the fake's config fields and `sample_config_options` (Task 3).
- Produces (`hennery_host::session`):
  - `pub struct Launch { request_id: String, session_id: String, attach: Attach, config: SessionConfig, agent: AgentCommand, cwd: PathBuf }`;
  - `pub fn launch(uplink: Uplink, launch: Launch, options: SessionOptions) -> SessionHandle`. `spawn` and `resume` keep their signatures and launch with an empty config;
  - `pub const CONFIG_TIMEOUT: Duration` (15 s) and `SessionOptions.config_timeout`.
- Produces (behaviour):
  - `initialize` advertises `session.configOptions.boolean` (decision 1). Without it, the fake offers `fast` as a select and the first test below fails;
  - the model and mode options found by decision 1's rule (`axis_id`, unit-tested in `session.rs`);
  - the pre-switch catalogue seeded from a `config_option_update` sent before the answer, if the answer has none (decision 4); after `session/new`, whatever the adapter sent before its answer is kept and emitted after `session_started`, like a load's kept updates;
  - after `session/new` or `session/load`, the switches of decision 2, under the start's one deadline, stopping at the first that does not answer, then the read-back comparison;
  - `session_started.indexed` holds the post-switch catalogue, or nothing if a switch left it unknown (decision 3);
  - then the replay's kept updates, the replay note, and at most one `host_note` whose `note` is `config_failed` (start) or `reapply_failed` (resume), scrubbed;
  - a repeated start announces the actor's current catalogue.
- Produces (`connection.rs`): `AttachRequest.config`; `start_session` / `resume_session` pass their config to `session::launch`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust

use hennery_host::outbox::Outbox;
use hennery_host::session::{self, AgentCommand, SessionCmd, SessionHandle, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{HostFrame, SessionBody, TurnOutcome};
use hennery_testkit::{FakeScript, SCRIPT_ENV, pid_alive};
```

with:

```rust

use hennery_host::outbox::Outbox;
use hennery_host::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, SessionBody, SessionConfig, TurnOutcome};
use hennery_testkit::{FakeScript, SCRIPT_ENV, pid_alive};
```

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
// Plan B2b: model, axes and mode (ACP core §4.3).

/// A fake with the sample catalogue whose model switch resets the mode,
/// logging every switch to `log`.
fn config_script(log: &Path) -> FakeScript {
    FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        model_switch_sets_mode: Some("default".into()),
        config_log: Some(log.to_string_lossy().into_owned()),
        ..FakeScript::default()
    }
}

fn wanted(model: Option<&str>, mode: Option<&str>, axes: &[(&str, ConfigValue)]) -> SessionConfig {
    SessionConfig {
        model: model.map(str::to_string),
        mode: mode.map(str::to_string),
        axes: axes.iter().map(|(id, v)| (id.to_string(), v.clone())).collect(),
    }
}

fn launching(
    uplink: &Uplink,
    script: &FakeScript,
    attach: Attach,
    config: SessionConfig,
    options: SessionOptions,
) -> SessionHandle {
    let launch = Launch {
        request_id: "r0".into(),
        session_id: "s1".into(),
        attach,
        config,
        agent: fake_with(script),
        cwd: std::env::temp_dir(),
    };
    session::launch(uplink.clone(), launch, options)
}

/// The catalogue extracts of the first `session_started`.
fn started_extracts(frames: &[HostFrame]) -> Indexed {
    frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(indexed.clone()),
            _ => None,
        })
        .expect("a session_started")
}

fn note_text(frames: &[HostFrame], code: &str) -> String {
    frames
        .iter()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::HostNote { note, text },
                ..
            } if note == code => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no host_note {code}: {:?}", kinds(frames)))
}

fn switches(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

#[tokio::test]
async fn a_start_applies_the_model_then_the_axes_then_the_mode_and_announces_the_result() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let config = wanted(
        Some("large"),
        Some("plan"),
        &[
            ("effort", ConfigValue::Id("high".into())),
            ("fast", ConfigValue::Bool(true)),
        ],
    );
    let _handle = launching(
        &uplink,
        &config_script(&log),
        Attach::New,
        config.clone(),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    // Mode last: the model switch reset it, and it is set again after.
    assert_eq!(switches(&log), "model=large\neffort=high\nfast=true\nmode=plan\n");
    let indexed = started_extracts(&frames);
    assert_eq!(indexed.current_config(), Some(config));
    assert_eq!(indexed.config_options.map(|o| o.len()), Some(4));
    assert_eq!(kinds(&frames), ["session_started"]);
}

#[tokio::test]
async fn values_already_current_are_not_switched_and_the_catalogue_is_still_announced() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let config = wanted(Some("small"), Some("default"), &[("fast", ConfigValue::Bool(false))]);
    let _handle = launching(
        &uplink,
        &config_script(&log),
        Attach::New,
        config.clone(),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(switches(&log), "");
    let current = started_extracts(&frames).current_config().unwrap();
    assert_eq!((current.model, current.mode), (config.model, config.mode));
}

/// A picker value the adapter does not offer (a stale catalogue) must not
/// cost the operator the session: it starts, and a note says what did not
/// take. The model the adapter refused is never announced as current.
#[tokio::test]
async fn a_start_whose_switches_fail_still_starts_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let config = wanted(Some("huge"), Some("plan"), &[("nope", ConfigValue::Id("x".into()))]);
    let _handle = launching(
        &uplink,
        &config_script(&log),
        Attach::New,
        config,
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(kinds(&frames), ["session_started", "host_note:config_failed"]);
    // The adapter refused `huge`; `nope` was never sent.
    assert_eq!(switches(&log), "model=huge\nmode=plan\n");
    let current = started_extracts(&frames).current_config().unwrap();
    assert_eq!(
        (current.model.as_deref(), current.mode.as_deref()),
        (Some("small"), Some("plan"))
    );
    let text = note_text(&frames, "config_failed");
    assert!(text.contains("model=huge") && text.contains("nope"), "{text}");
}

#[tokio::test]
async fn a_resume_re_applies_the_stored_config_and_a_failed_re_apply_is_only_a_note() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        replay: vec![json!({"sessionUpdate": "available_commands_update", "availableCommands": []})],
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
        wanted(Some("huge"), Some("bypass"), &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("host_note:reapply_failed")).await;
    assert_eq!(
        kinds(&frames),
        ["session_started", "update:?", "host_note:reapply_failed"]
    );
    assert_eq!(
        started_extracts(&frames).current_mode.as_deref(),
        Some("bypass"),
        "the mode survived the resume"
    );
    // Still attached: the failed re-apply did not fail the resume.
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
}

/// An adapter that answers a switch without a catalogue, or never answers,
/// leaves hennery not knowing the current values: it announces none, so
/// the collector keeps what it stored (a resume's stored mode is not
/// overwritten with a guess).
#[tokio::test]
async fn a_switch_without_a_read_back_announces_no_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        empty_config_read_back: true,
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), None, &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(switches(&log), "model=large\n");
    assert_eq!(started_extracts(&frames), Indexed::default());
    assert_eq!(kinds(&frames), ["session_started"]);
}

#[tokio::test]
async fn a_hung_switch_is_reported_and_the_session_still_starts() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("plan"), &[]),
        SessionOptions {
            config_timeout: Duration::from_millis(200),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(kinds(&frames), ["session_started", "host_note:config_failed"]);
    assert_eq!(started_extracts(&frames), Indexed::default());
    let text = note_text(&frames, "config_failed");
    assert!(text.contains("model=large: no answer within"), "{text}");
    // Nothing is sent after a switch that did not answer.
    assert!(
        text.contains("mode=plan: not sent: an earlier switch did not answer"),
        "{text}"
    );
    assert_eq!(switches(&log), "model=large\n");
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_ended")).await;
}

/// The real adapters handle requests concurrently. A model switch that
/// answers after its timeout can still land, and clamp the mode, after a
/// mode switch sent behind it has answered: model first and mode last would
/// silently break. So nothing more is sent once a switch has not answered.
#[tokio::test]
async fn a_late_model_switch_stops_the_starts_switches_so_its_clamp_cannot_undo_the_mode() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        slow_model_switch_ms: Some(600),
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("plan"), &[]),
        SessionOptions {
            config_timeout: Duration::from_millis(200),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    // Give a wrongly sent mode switch time to reach the fake.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        switches(&log),
        "model=large\n",
        "the mode switch went out behind a late model switch"
    );
    assert_eq!(started_extracts(&frames), Indexed::default(), "the values are unknown");
    let text = note_text(&frames, "config_failed");
    assert!(
        text.contains("mode=plan: not sent: an earlier switch did not answer"),
        "{text}"
    );
}

#[tokio::test]
async fn a_switch_cut_off_by_the_start_deadline_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(Some("large"), Some("plan"), &[]),
        SessionOptions {
            start_timeout: Duration::from_secs(1),
            config_timeout: Duration::from_secs(10),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(kinds(&frames), ["session_started", "host_note:config_failed"]);
    let text = note_text(&frames, "config_failed");
    assert!(
        text.contains("model=large: no answer before the start deadline"),
        "{text}"
    );
    assert!(text.contains("mode=plan: not sent"), "{text}");
}

/// An adapter may accept a value and report another. What it reports is
/// what counts, so the note says so.
#[tokio::test]
async fn a_value_the_agent_accepts_but_does_not_report_is_noted() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        sticky_options: vec!["effort".into()],
        ..config_script(&log)
    };
    let _handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(None, None, &[("effort", ConfigValue::Id("high".into()))]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("host_note:config_failed")).await;
    assert_eq!(switches(&log), "effort=high\n");
    let text = note_text(&frames, "config_failed");
    assert!(text.contains("effort: asked high, agent reports low"), "{text}");
    let axes = started_extracts(&frames).current_axes.unwrap();
    assert_eq!(axes.get("effort"), Some(&ConfigValue::Id("low".into())));
}

/// An adapter that announces its options in a `config_option_update` just
/// before answering `session/new` or `session/load`, not in the answer:
/// those options are the ones switched, and that update, older than the
/// announced catalogue, carries none.
#[tokio::test]
async fn options_announced_in_an_update_before_the_answer_are_the_ones_switched() {
    for attach in [
        Attach::New,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
    ] {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("config.log");
        let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        let script = FakeScript {
            config_in_update_only: true,
            ..config_script(&log)
        };
        let _handle = launching(
            &uplink,
            &script,
            attach.clone(),
            wanted(None, Some("plan"), &[]),
            SessionOptions::default(),
        );
        let frames = wait_until(&uplink, has("update:?")).await;
        assert_eq!(kinds(&frames), ["session_started", "update:?"], "{attach:?}");
        assert_eq!(switches(&log), "mode=plan\n", "{attach:?}");
        assert_eq!(started_extracts(&frames).current_mode.as_deref(), Some("plan"));
        let HostFrame::Session {
            body: SessionBody::AcpUpdate { indexed, .. },
            ..
        } = &frames[1]
        else {
            panic!("{frames:?}");
        };
        assert_eq!(*indexed, Indexed::default(), "{attach:?}");
    }
}
```

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
// Plan B2b: config over the connection.

fn configurable_fake() -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

fn current_mode(frame: &HostFrame) -> Option<String> {
    match frame {
        HostFrame::Session {
            body: hennery_proto::frames::SessionBody::SessionStarted { indexed, .. },
            ..
        } => indexed.current_mode.clone(),
        other => panic!("expected session_started, got {other:?}"),
    }
}

#[tokio::test]
async fn start_and_resume_carry_their_config_to_the_adapter() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "start-config", configurable_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    let plan = hennery_proto::frames::SessionConfig {
        mode: Some("plan".into()),
        ..Default::default()
    };
    let mut frame = start("r1", "s1");
    if let CollectorFrame::StartSession { config, .. } = &mut frame {
        *config = plan.clone();
    }
    send_frame(&mut sink, &frame).await;
    let started = read_until(&mut stream, body_is("s1", "session_started")).await;
    assert_eq!(current_mode(&started).as_deref(), Some("plan"));

    let mut frame = resume("r2", "s2", 0, "agent-7");
    if let CollectorFrame::ResumeSession { config, .. } = &mut frame {
        *config = hennery_proto::frames::SessionConfig {
            mode: Some("bypass".into()),
            ..Default::default()
        };
    }
    send_frame(&mut sink, &frame).await;
    let resumed = read_until(&mut stream, body_is("s2", "session_started")).await;
    assert_eq!(current_mode(&resumed).as_deref(), Some("bypass"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session --test host_connection`
Expected: does not compile: `unresolved import 'hennery_host::session::Launch'`, `cannot find function 'launch' in module 'session'`, and `struct 'SessionOptions' has no field named 'config_timeout'`.

- [ ] **Step 3: Implement the switches in the session actor**

In `crates/hennery-host/src/session.rs`, replace:

```rust
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    PromptResponse, SessionId, StopReason,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, UntypedMessage};
use hennery_proto::frames::{HostFrame, Indexed, ParkReason, SessionBody, TurnOutcome};
use serde_json::Value;
```

with:

```rust
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    BooleanConfigOptionCapabilities, CancelNotification, ClientCapabilities, ClientSessionCapabilities, ContentBlock,
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigOptionsCapabilities,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, StopReason,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, UntypedMessage};
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome};
use serde_json::Value;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// drop the whole host connection.
pub const CANCEL_GRACE: Duration = Duration::from_secs(20);

```

with:

```rust
/// drop the whole host connection.
pub const CANCEL_GRACE: Duration = Duration::from_secs(20);

/// How long one `session/set_config_option` may take. A start's or a
/// resume's switch that takes longer is reported, not fatal (ACP core
/// §4.3); a `set_config` that takes longer is answered `config_failed`,
/// well before the collector's 60 s timeout (§3.4).
pub const CONFIG_TIMEOUT: Duration = Duration::from_secs(15);

```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// How long a cancelled turn may run on before the adapter is stopped.
    pub cancel_grace: Duration,
}
```

with:

```rust
    /// How long a cancelled turn may run on before the adapter is stopped.
    pub cancel_grace: Duration,
    /// How long one config switch may take.
    pub config_timeout: Duration,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            idle_timeout: Some(IDLE_TIMEOUT),
            cancel_grace: CANCEL_GRACE,
        }
    }
}
```

with:

```rust
            idle_timeout: Some(IDLE_TIMEOUT),
            cancel_grace: CANCEL_GRACE,
            config_timeout: CONFIG_TIMEOUT,
        }
    }
}

/// Everything a session actor needs to attach its session.
#[derive(Debug, Clone)]
pub struct Launch {
    pub request_id: String,
    pub session_id: String,
    pub attach: Attach,
    /// Applied once the adapter session exists (ACP core §4.3).
    pub config: SessionConfig,
    pub agent: AgentCommand,
    pub cwd: PathBuf,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    options: SessionOptions,
) -> SessionHandle {
    spawn_actor(uplink, request_id, session_id, Attach::New, agent, cwd, options)
}
```

with:

```rust
    options: SessionOptions,
) -> SessionHandle {
    let launch = Launch {
        request_id,
        session_id,
        attach: Attach::New,
        config: SessionConfig::default(),
        agent,
        cwd,
    };
    self::launch(uplink, launch, options)
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    options: SessionOptions,
) -> SessionHandle {
    let attach = Attach::Load { agent_session_id };
    spawn_actor(uplink, request_id, session_id, attach, agent, cwd, options)
}

fn spawn_actor(
    uplink: Uplink,
    request_id: String,
    session_id: String,
    attach: Attach,
    agent: AgentCommand,
    cwd: PathBuf,
    options: SessionOptions,
) -> SessionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
```

with:

```rust
    options: SessionOptions,
) -> SessionHandle {
    let launch = Launch {
        request_id,
        session_id,
        attach: Attach::Load { agent_session_id },
        config: SessionConfig::default(),
        agent,
        cwd,
    };
    self::launch(uplink, launch, options)
}

/// Spawn a session actor for `launch`: `session/new` or `session/load`,
/// then its config. Otherwise like [`spawn`].
pub fn launch(uplink: Uplink, launch: Launch, options: SessionOptions) -> SessionHandle {
    let (tx, rx) = mpsc::unbounded_channel();
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    let actor = Actor {
        uplink,
        session_id,
        open_turn: open_turn.clone(),
        options,
    };
```

with:

```rust
    let actor = Actor {
        uplink,
        session_id: launch.session_id.clone(),
        open_turn: open_turn.clone(),
        options,
        catalogue: Mutex::new(Catalogue::default()),
    };
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    tokio::spawn(async move {
        let _finished = finished;
        actor.run(request_id, attach, agent, cwd, rx).await;
    });
```

with:

```rust
    tokio::spawn(async move {
        let _finished = finished;
        actor.run(launch, rx).await;
    });
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    open_turn: Arc<Mutex<Option<String>>>,
    options: SessionOptions,
}

impl Actor {
    fn emit(&self, body: SessionBody) {
```

with:

```rust
    open_turn: Arc<Mutex<Option<String>>>,
    options: SessionOptions,
    /// The adapter's config options as last reported.
    catalogue: Mutex<Catalogue>,
}

/// What the actor knows of its adapter's config options.
#[derive(Default)]
struct Catalogue {
    options: Vec<SessionConfigOption>,
    /// `false` once a switch went unanswered or was answered without a
    /// catalogue: the options may be stale, so they are not announced
    /// until the adapter reports them again.
    current: bool,
}

impl Actor {
    /// The catalogue extracts to announce: none while the options may be
    /// stale.
    fn catalogue_extracts(&self) -> Indexed {
        let catalogue = self.catalogue.lock().expect("catalogue lock");
        if catalogue.current {
            catalogue_extracts(&catalogue.options)
        } else {
            Indexed::default()
        }
    }

    fn emit(&self, body: SessionBody) {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    }

    async fn run(
        self,
        request_id: String,
        attach: Attach,
        agent: AgentCommand,
        cwd: PathBuf,
        mut commands: mpsc::UnboundedReceiver<SessionCmd>,
    ) {
        self.drive(request_id, attach, agent, cwd, &mut commands).await;
        // The session's last frame is in the outbox. Commands sent while the
```

with:

```rust
    }

    async fn run(self, launch: Launch, mut commands: mpsc::UnboundedReceiver<SessionCmd>) {
        self.drive(launch, &mut commands).await;
        // The session's last frame is in the outbox. Commands sent while the
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The actor's life: start, then serve until it parks, closes or ends.
    /// Every return leaves the session's final frame in the outbox.
    async fn drive(
        &self,
        request_id: String,
        attach: Attach,
        agent: AgentCommand,
        cwd: PathBuf,
        commands: &mut mpsc::UnboundedReceiver<SessionCmd>,
    ) {
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
```

with:

```rust
    /// The actor's life: start, then serve until it parks, closes or ends.
    /// Every return leaves the session's final frame in the outbox.
    async fn drive(&self, launch: Launch, commands: &mut mpsc::UnboundedReceiver<SessionCmd>) {
        let Launch {
            request_id,
            attach,
            config,
            agent,
            cwd,
            ..
        } = launch;
        let (mut adapter, io) = match Adapter::spawn(&agent, &cwd) {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            );
        };
        let negotiated = tokio::select! {
            result = tokio::time::timeout(self.options.start_timeout, negotiate(&conn, cwd, &attach, &mut updates)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(StartError::other(format!(
                        "adapter did not start within {}s",
                        self.options.start_timeout.as_secs()
                    ))),
                }
            }
            info = adapter.exited() => {
```

with:

```rust
            );
        };
        // One deadline for the whole start: the switches share it, so a slow
        // one cannot push the answer past the collector's start timeout.
        let deadline = Instant::now() + self.options.start_timeout;
        let started = tokio::select! {
            result = async {
                let (session, replay, catalogue) =
                    match tokio::time::timeout_at(deadline, negotiate(&conn, cwd, &attach, &mut updates)).await {
                        Ok(result) => result?,
                        Err(_) => {
                            return Err(StartError::other(format!(
                                "adapter did not start within {}s",
                                self.options.start_timeout.as_secs()
                            )));
                        }
                    };
                let applied = apply_config(&conn, &session, catalogue, &config, self.options.config_timeout, deadline).await;
                Ok((session, replay, applied))
            } => result,
            info = adapter.exited() => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            }
        };
        let (agent_session, replay) = match negotiated {
            Ok(negotiated) => negotiated,
            Err(error) => {
```

with:

```rust
            }
        };
        let (agent_session, replay, applied) = match started {
            Ok(started) => started,
            Err(error) => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            }
        };
        self.emit(SessionBody::SessionStarted {
            request_id,
            agent_session_id: agent_session.to_string(),
            indexed: Indexed::default(),
        });
        // A load's state updates follow the start they belong to; then the
        // note about what the load dropped (ACP core §4.5).
        for payload in replay.kept.iter().cloned() {
```

with:

```rust
            }
        };
        *self.catalogue.lock().expect("catalogue lock") = Catalogue {
            options: applied.options,
            current: applied.current,
        };
        // The catalogue after the switches, never the one before (P-13).
        self.emit(SessionBody::SessionStarted {
            request_id,
            agent_session_id: agent_session.to_string(),
            indexed: self.catalogue_extracts(),
        });
        // A load's state updates follow the start they belong to; then the
        // note about what the load dropped (ACP core §4.5), then the one
        // about switches that did not take.
        for payload in replay.kept.iter().cloned() {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        if let Some(note) = replay.note() {
            self.emit(note);
        }
```

with:

```rust
        if let Some(note) = replay.note() {
            self.emit(note);
        }
        if !applied.failures.is_empty() {
            let (note, what) = match attach {
                Attach::New => ("config_failed", "could not apply"),
                Attach::Load { .. } => ("reapply_failed", "could not re-apply"),
            };
            self.emit(SessionBody::HostNote {
                note: note.into(),
                text: scrub(&format!("{what}: {}", applied.failures.join("; "))),
            });
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                        request_id,
                        agent_session_id: agent_session.to_string(),
                        indexed: Indexed::default(),
                    }),
```

with:

```rust
                        request_id,
                        agent_session_id: agent_session.to_string(),
                        indexed: self.catalogue_extracts(),
                    }),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// `initialize`, then `session/new` or `session/load`. While a load is
/// outstanding its replay is classified, never emitted (ACP core §4.5).
async fn negotiate(
```

with:

```rust
/// `initialize`, then `session/new` or `session/load`. While a load is
/// outstanding its replay is classified, never emitted (ACP core §4.5).
/// Returns the config options the adapter announced (none if it announced
/// none, or they did not parse).
async fn negotiate(
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    attach: &Attach,
    updates: &mut mpsc::UnboundedReceiver<Value>,
) -> Result<(SessionId, Replay), StartError> {
    let init = conn
        .send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
```

with:

```rust
    attach: &Attach,
    updates: &mut mpsc::UnboundedReceiver<Value>,
) -> Result<(SessionId, Replay, Vec<SessionConfigOption>), StartError> {
    // Advertised so that agents offer boolean options as booleans, not as
    // on/off selects (ACP `session.configOptions.boolean`).
    let capabilities = ClientCapabilities::new().session(
        ClientSessionCapabilities::new()
            .config_options(SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new())),
    );
    let init = conn
        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(capabilities))
        .block_task()
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                .await
                .map_err(|err| StartError::acp(err, false))?;
            return Ok((created.session_id, Replay::default()));
        }
```

with:

```rust
                .await
                .map_err(|err| StartError::acp(err, false))?;
            // What the adapter sent before its answer (see
            // `Actor::drain_updates` for the ordering argument) follows the
            // start, like a load's kept updates.
            let mut replay = Replay::default();
            for _ in 0..updates.len() {
                match updates.try_recv() {
                    Ok(payload) => replay.kept.push(payload),
                    Err(_) => break,
                }
            }
            let options = announced_options(created.config_options, &replay.kept);
            return Ok((created.session_id, replay, options));
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    replay.observe(payload);
                }
                result.map_err(|err| StartError::acp(err, true))?;
                return Ok((id, replay));
            }
        }
    }
```

with:

```rust
                    replay.observe(payload);
                }
                let loaded = result.map_err(|err| StartError::acp(err, true))?;
                let options = announced_options(loaded.config_options, &replay.kept);
                return Ok((id, replay, options));
            }
        }
    }
}

/// The config options a new or loaded session starts with: the answer's,
/// or, if it had none, those of the last `config_option_update` the adapter
/// sent before it. That update still carries no extracts: it is older than
/// the catalogue `session_started` announces.
fn announced_options(answered: Option<Vec<SessionConfigOption>>, before: &[Value]) -> Vec<SessionConfigOption> {
    match answered {
        Some(options) if !options.is_empty() => options,
        _ => before.iter().rev().find_map(config_update).unwrap_or_default(),
    }
}

/// The options of a non-empty `config_option_update` notification.
fn config_update(payload: &Value) -> Option<Vec<SessionConfigOption>> {
    let notification = serde_json::from_value::<SessionNotification>(payload.clone()).ok()?;
    match notification.update {
        SessionUpdate::ConfigOptionUpdate(update) if !update.config_options.is_empty() => Some(update.config_options),
        _ => None,
    }
}

/// What a start's config switches left behind.
struct Applied {
    /// The options the last switch answered with (or the announced ones).
    options: Vec<SessionConfigOption>,
    /// `false` if a switch went unanswered or was answered without a
    /// catalogue: `options` may then be stale.
    current: bool,
    /// One line per requested value that did not take.
    failures: Vec<String>,
}

/// Apply `wanted` to a new or loaded session (ACP core §4.3): the model
/// first, then the other axes, then the mode, each with
/// `session/set_config_option`, so a model that clamps the mode cannot undo
/// the requested mode. A value that is already current is not sent. Each
/// switch has `timeout`, and none runs past `deadline`.
///
/// Once a switch goes unanswered (or the deadline has passed), nothing more
/// is sent: the adapter may still apply the late switch, and a late model
/// switch could clamp a mode sent after it. Finally every requested value is
/// checked against the read-back. Whatever did not take is reported in
/// `failures`; the start goes on.
async fn apply_config(
    conn: &ConnectionTo<Agent>,
    session: &SessionId,
    options: Vec<SessionConfigOption>,
    wanted: &SessionConfig,
    timeout: Duration,
    deadline: Instant,
) -> Applied {
    let mut applied = Applied {
        options,
        current: true,
        failures: Vec::new(),
    };
    let mut switches: Vec<(String, ConfigValue)> = Vec::new();
    if let Some(model) = &wanted.model {
        match axis_id(&applied.options, &SessionConfigOptionCategory::Model, "model") {
            Some(id) => switches.push((id, ConfigValue::Id(model.clone()))),
            None => applied
                .failures
                .push(format!("model {model}: the adapter offers no model option")),
        }
    }
    switches.extend(wanted.axes.iter().map(|(id, value)| (id.clone(), value.clone())));
    // Resolved now, and applied last: after the model and the other axes.
    let mode = wanted.mode.as_ref().map(|mode| {
        (
            axis_id(&applied.options, &SessionConfigOptionCategory::Mode, "mode"),
            mode,
        )
    });
    match mode {
        Some((Some(id), mode)) => switches.push((id, ConfigValue::Id(mode.clone()))),
        Some((None, mode)) => applied
            .failures
            .push(format!("mode {mode}: the adapter offers no mode option")),
        None => {}
    }
    // Requested values that already have their own line in `failures`.
    let mut reported: HashSet<String> = HashSet::new();
    let mut unanswered = false;
    for (id, value) in &switches {
        match current_value(&applied.options, id) {
            None => {
                applied
                    .failures
                    .push(format!("{id}: the adapter offers no such option"));
                reported.insert(id.clone());
                continue;
            }
            Some(current) if current == *value => continue,
            Some(_) => {}
        }
        let now = Instant::now();
        if unanswered || now >= deadline {
            let why = if unanswered {
                "an earlier switch did not answer"
            } else {
                "the start deadline passed"
            };
            applied.failures.push(format!("{id}={}: not sent: {why}", shown(value)));
            reported.insert(id.clone());
            continue;
        }
        let request = SetSessionConfigOptionRequest::new(session.clone(), id.clone(), acp_value(value));
        let cut_short = now + timeout > deadline;
        let until = (now + timeout).min(deadline);
        match tokio::time::timeout_at(until, conn.send_request(request).block_task()).await {
            // The catalogue of the last successful switch is the one kept.
            Ok(Ok(response)) if !response.config_options.is_empty() => applied.options = response.config_options,
            // An empty answer is no read-back, not an adapter without options.
            Ok(Ok(_)) => applied.current = false,
            Ok(Err(err)) => {
                applied.failures.push(format!("{id}={}: {err}", shown(value)));
                reported.insert(id.clone());
            }
            Err(_) => {
                applied.current = false;
                unanswered = true;
                let wait = if cut_short {
                    "no answer before the start deadline".to_string()
                } else {
                    format!("no answer within {timeout:?}")
                };
                applied.failures.push(format!("{id}={}: {wait}", shown(value)));
                reported.insert(id.clone());
            }
        }
    }
    // What the agent reports is what counts: a value it accepted but does
    // not report (or a later switch undid) did not take either.
    if applied.current {
        for (id, value) in switches.iter().filter(|(id, _)| !reported.contains(id)) {
            match current_value(&applied.options, id) {
                Some(current) if current == *value => {}
                Some(current) => applied.failures.push(format!(
                    "{id}: asked {}, agent reports {}",
                    shown(value),
                    shown(&current)
                )),
                None => applied
                    .failures
                    .push(format!("{id}: asked {}, agent no longer offers it", shown(value))),
            }
        }
    }
    applied
}

/// The id of the option for `category`. Categories are only a UX hint in
/// ACP, so an option with the conventional id counts too, but only if it
/// has no category, or a custom (`_`-prefixed) one: an option another
/// category claims is not the model or the mode.
fn axis_id(options: &[SessionConfigOption], category: &SessionConfigOptionCategory, id: &str) -> Option<String> {
    let uncategorized = |o: &&SessionConfigOption| match &o.category {
        None => true,
        Some(SessionConfigOptionCategory::Other(custom)) => custom.starts_with('_'),
        Some(_) => false,
    };
    options
        .iter()
        .find(|o| o.category.as_ref() == Some(category))
        .or_else(|| options.iter().filter(uncategorized).find(|o| &*o.id.0 == id))
        .map(|o| o.id.to_string())
}

/// The current value of option `id`, if the adapter offers it.
fn current_value(options: &[SessionConfigOption], id: &str) -> Option<ConfigValue> {
    let option = options.iter().find(|o| &*o.id.0 == id)?;
    match &option.kind {
        SessionConfigKind::Select(select) => Some(ConfigValue::Id(select.current_value.to_string())),
        SessionConfigKind::Boolean(toggle) => Some(ConfigValue::Bool(toggle.current_value)),
        _ => None,
    }
}

fn acp_value(value: &ConfigValue) -> SessionConfigOptionValue {
    match value {
        ConfigValue::Bool(on) => SessionConfigOptionValue::boolean(*on),
        ConfigValue::Id(id) => SessionConfigOptionValue::value_id(id.clone()),
    }
}

fn shown(value: &ConfigValue) -> String {
    match value {
        ConfigValue::Bool(on) => on.to_string(),
        ConfigValue::Id(id) => id.clone(),
    }
}

/// The catalogue extracts (ACP core §3.2) of `options`: the options
/// themselves, and the current model, mode and other axes. None for an
/// empty list, which is no read-back.
fn catalogue_extracts(options: &[SessionConfigOption]) -> Indexed {
    if options.is_empty() {
        return Indexed::default();
    }
    let model = axis_id(options, &SessionConfigOptionCategory::Model, "model");
    let mode = axis_id(options, &SessionConfigOptionCategory::Mode, "mode");
    let id_of = |id: &Option<String>| id.as_deref().and_then(|id| current_value(options, id));
    let as_id = |value: Option<ConfigValue>| match value {
        Some(ConfigValue::Id(id)) => Some(id),
        _ => None,
    };
    let axes = options
        .iter()
        .map(|o| o.id.to_string())
        .filter(|id| Some(id) != model.as_ref() && Some(id) != mode.as_ref())
        .filter_map(|id| current_value(options, &id).map(|value| (id, value)))
        .collect();
    Indexed {
        config_options: Some(options.iter().filter_map(|o| serde_json::to_value(o).ok()).collect()),
        current_model: as_id(id_of(&model)),
        current_mode: as_id(id_of(&mode)),
        current_axes: Some(axes),
        ..Indexed::default()
    }
```

Append to `crates/hennery-host/src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn options(json: Value) -> Vec<SessionConfigOption> {
        serde_json::from_value(json).unwrap()
    }

    fn select(id: &str, category: Option<&str>) -> Value {
        let mut option = serde_json::json!({
            "id": id, "name": id, "type": "select", "currentValue": "a",
            "options": [{"value": "a", "name": "a"}]
        });
        if let Some(category) = category {
            option["category"] = category.into();
        }
        option
    }

    #[test]
    fn the_model_is_its_category_or_else_an_uncategorized_option_named_model() {
        let model = SessionConfigOptionCategory::Model;
        let pick = |json| axis_id(&options(json), &model, "model");
        let categorized = serde_json::json!([select("model", Some("thought_level")), select("brain", Some("model"))]);
        assert_eq!(pick(categorized).as_deref(), Some("brain"));
        assert_eq!(
            pick(serde_json::json!([select("model", None)])).as_deref(),
            Some("model")
        );
        assert_eq!(
            pick(serde_json::json!([select("model", Some("_mine"))])).as_deref(),
            Some("model")
        );
        // Another category claims it, even one this build does not know.
        assert_eq!(pick(serde_json::json!([select("model", Some("thought_level"))])), None);
        assert_eq!(pick(serde_json::json!([select("model", Some("future"))])), None);
    }
}
```

- [ ] **Step 4: Pass the config through the connection**

In `crates/hennery-host/src/connection.rs`, replace:

```rust

use crate::outbox::Outbox;
use crate::session::{self, AgentCommand, Attach, SessionCmd, SessionHandle, SessionOptions};
use crate::uplink::Uplink;
```

with:

```rust

use crate::outbox::Outbox;
use crate::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use crate::uplink::Uplink;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
use futures::{SinkExt, StreamExt};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame};
use std::collections::HashMap;
```

with:

```rust
use futures::{SinkExt, StreamExt};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionConfig};
use std::collections::HashMap;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    cwd: String,
    attach: Attach,
}
```

with:

```rust
    cwd: String,
    attach: Attach,
    config: SessionConfig,
}
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        return;
    }
    let cwd = PathBuf::from(req.cwd);
    let handle = match req.attach {
        Attach::New => session::spawn(
            uplink.clone(),
            req.request_id,
            req.session_id.clone(),
            command,
            cwd,
            options,
        ),
        Attach::Load { agent_session_id } => session::resume(
            uplink.clone(),
            req.request_id,
            req.session_id.clone(),
            agent_session_id,
            command,
            cwd,
            options,
        ),
    };
    map.handles.insert(req.session_id, handle);
}
```

with:

```rust
        return;
    }
    let launch = Launch {
        request_id: req.request_id,
        session_id: req.session_id.clone(),
        attach: req.attach,
        config: req.config,
        agent: command,
        cwd: PathBuf::from(req.cwd),
    };
    map.handles
        .insert(req.session_id, session::launch(uplink.clone(), launch, options));
}
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            agent,
            cwd,
            ..
        } => attach(
```

with:

```rust
            agent,
            cwd,
            config,
        } => attach(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
                cwd,
                attach: Attach::New,
            },
```

with:

```rust
                cwd,
                attach: Attach::New,
                config,
            },
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            cwd,
            agent_session_id,
            ..
        } => attach(
```

with:

```rust
            cwd,
            agent_session_id,
            config,
        } => attach(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
                cwd,
                attach: Attach::Load { agent_session_id },
            },
```

with:

```rust
                cwd,
                attach: Attach::Load { agent_session_id },
                config,
            },
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_session --test host_connection`
Expected: `host_session` 40 pass, `host_connection` 16 pass, and `cargo test -p hennery-host --lib` 2 pass. The two B1 resume tests that count frames after a load still pass: without a requested config nothing is switched, and no note is added.

- [ ] **Step 6: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-host crates/hennery-testkit
git commit -m "feat(host): apply model, axes and mode on start and resume"
```

Expected: 219 tests pass.

---

### Task 5: Host: `set_config` and the live catalogue

**Files:**
- Modify: `crates/hennery-host/src/session.rs`, `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`, `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Consumes: `Launch`, `launch`, the actor's `Catalogue` (Task 4); `CollectorFrame::SetConfig`, `SessionBody::ConfigApplied` (Task 2).
- Produces: `SessionCmd::SetConfig { request_id: String, config_id: String, value: ConfigValue }`. The actor queues it and sends one switch at a time (decision 6); its deadline is its receipt plus `config_timeout`. It is answered by `config_applied` (with the read-back's extracts, or none for an empty read-back), or by `error` with code:
  - `unknown_option`, `invalid` (checked on the host);
  - `config_failed` (the adapter refused; no answer by the deadline; or the deadline passed while it waited: `an earlier switch is still out`);
  - `not_attached` (still waiting or out when the actor ended).
- Produces (behaviour):
  - a live `config_option_update` replaces the actor's catalogue and carries the catalogue extracts;
  - updates queued before the main loop (the load's kept ones, and any sent during the switches) carry none;
  - in `connection.rs`, `set_config` goes to the session's actor, or answers `not_attached` (this replaces Task 2's stub).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
fn set_config(request_id: &str, config_id: &str, value: ConfigValue) -> SessionCmd {
    SessionCmd::SetConfig {
        request_id: request_id.into(),
        config_id: config_id.into(),
        value,
    }
}

/// The catalogue extracts of every `config_applied`, with its request id.
fn applied(frames: &[HostFrame]) -> Vec<(String, Indexed)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::ConfigApplied { request_id, indexed },
                ..
            } => Some((request_id.clone(), indexed.clone())),
            _ => None,
        })
        .collect()
}

async fn refusal(replies: &mut tokio::sync::mpsc::UnboundedReceiver<HostFrame>) -> (String, String) {
    match tokio::time::timeout(Duration::from_secs(10), replies.recv())
        .await
        .expect("a reply, not silence")
        .unwrap()
    {
        HostFrame::Error { request_id, code, .. } => (request_id, code),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn set_config_switches_during_a_turn_and_answers_with_the_adapters_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: (1..=5).map(|n| n.to_string()).collect(),
        chunk_delay_ms: 100,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        wanted(None, Some("plan"), &[]),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    // The model switch clamps the mode: the read-back says so.
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    let frames = wait_until(&uplink, has("config_applied")).await;
    let kinds = kinds(&frames);
    let at = kinds.iter().position(|k| k == "config_applied").unwrap();
    assert!(
        at < kinds.iter().position(|k| k == "turn_ended").unwrap_or(usize::MAX),
        "the switch waited for the turn: {kinds:?}"
    );
    let (request, indexed) = applied(&frames).remove(0);
    let current = indexed.current_config().unwrap();
    assert_eq!(
        (request.as_str(), current.model.as_deref(), current.mode.as_deref()),
        ("rc1", Some("large"), Some("default"))
    );
    // A value the adapter refuses, an option it does not have, a value of
    // the wrong kind: each is answered, and only the first reaches it.
    assert!(handle.send(set_config("rc2", "model", ConfigValue::Id("huge".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc2".to_string(), "config_failed".to_string())
    );
    assert!(handle.send(set_config("rc3", "nope", ConfigValue::Id("x".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc3".to_string(), "unknown_option".to_string())
    );
    assert!(handle.send(set_config("rc4", "fast", ConfigValue::Id("x".into()))));
    assert_eq!(refusal(&mut replies).await, ("rc4".to_string(), "invalid".to_string()));
    assert_eq!(switches(&log), "mode=plan\nmodel=large\nmodel=huge\n");
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(applied(&frames).len(), 1);
}

/// P-13 at the source: a `config_option_update` replayed by `session/load`
/// predates the switches, so it must not carry a catalogue that could
/// overwrite the announced one. One the agent sends live (it left plan mode
/// on its own) must: that is how its new mode gets stored.
#[tokio::test]
async fn only_a_live_config_option_update_carries_the_catalogue() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let mut stale = hennery_testkit::sample_config_options();
    stale[0]["currentValue"] = json!("large");
    let script = FakeScript {
        replay: vec![json!({"sessionUpdate": "config_option_update", "configOptions": stale})],
        prompt_sets_mode: Some("bypass".into()),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::Load {
            agent_session_id: "agent-7".into(),
        },
        wanted(None, Some("plan"), &[]),
        SessionOptions::default(),
    );
    let frames = wait_until(&uplink, has("session_started")).await;
    assert_eq!(kinds(&frames), ["session_started", "update:?"]);
    assert_eq!(started_extracts(&frames).current_mode.as_deref(), Some("plan"));
    let replayed = match &frames[1] {
        HostFrame::Session {
            body: SessionBody::AcpUpdate { indexed, .. },
            ..
        } => indexed.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(replayed, Indexed::default(), "a replayed update carried a catalogue");

    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let live: Vec<Indexed> = frames
        .iter()
        .skip(2)
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::AcpUpdate { indexed, .. },
                ..
            } if indexed.config_options.is_some() => Some(indexed.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(live.len(), 1, "{:?}", kinds(&frames));
    assert_eq!(live[0].current_mode.as_deref(), Some("bypass"));
    assert_eq!(live[0].turn_id.as_deref(), Some("t1"));
    // A repeated start announces what the agent last reported.
    assert!(handle.send(SessionCmd::Restart {
        request_id: "r9".into()
    }));
    let frames = wait_until(&uplink, |f| {
        kinds(f).iter().filter(|k| *k == "session_started").count() == 2
    })
    .await;
    let last = frames
        .iter()
        .rev()
        .find_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::SessionStarted { indexed, .. },
                ..
            } => Some(indexed.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.current_mode.as_deref(), Some("bypass"));
}

#[tokio::test]
async fn a_switch_that_never_answers_is_config_failed_and_one_still_out_at_the_end_is_not_attached() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(300),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc1".to_string(), "config_failed".to_string())
    );
    // Parked while a switch is out: it is answered, after the park.
    assert!(handle.send(set_config("rc2", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(SessionCmd::Park {
        request_id: "rp".into()
    }));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc2".to_string(), "not_attached".to_string())
    );
    wait_until(&uplink, has("session_parked:operator")).await;
    assert!(applied(&uplink.pending().unwrap()).is_empty());
}

/// The real adapters handle requests concurrently. If both of these went
/// out at once, the slow model switch would answer last and then clamp the
/// mode the second switch had just set, while hennery showed `plan`. One
/// switch at a time keeps them in the order the operator made them.
#[tokio::test]
async fn set_config_sends_one_switch_at_a_time_so_a_late_clamp_cannot_undo_a_later_switch() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        slow_model_switch_ms: Some(300),
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions::default(),
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "mode", ConfigValue::Id("plan".into()))));
    let frames = wait_until(&uplink, |f| applied(f).len() == 2).await;
    let answers: Vec<String> = applied(&frames).into_iter().map(|(r, _)| r).collect();
    assert_eq!(answers, ["rc1", "rc2"]);
    // A third switch reads back the agent's real state.
    assert!(handle.send(set_config("rc3", "effort", ConfigValue::Id("high".into()))));
    let frames = wait_until(&uplink, |f| applied(f).len() == 3).await;
    let (_, last) = applied(&frames).remove(2);
    let current = last.current_config().unwrap();
    assert_eq!(
        (current.model.as_deref(), current.mode.as_deref()),
        (Some("large"), Some("plan")),
        "the late clamp undid the later switch"
    );
}

/// A switch's deadline runs from when the host received it, not from when
/// it could be sent: one waiting behind a hung switch does not get a fresh
/// `config_timeout` of its own.
#[tokio::test]
async fn a_switch_waiting_behind_a_hung_one_is_answered_by_its_own_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let (uplink, mut replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        hang_config: true,
        ..config_script(&log)
    };
    let handle = launching(
        &uplink,
        &script,
        Attach::New,
        SessionConfig::default(),
        SessionOptions {
            config_timeout: Duration::from_millis(300),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    let began = std::time::Instant::now();
    assert!(handle.send(set_config("rc1", "model", ConfigValue::Id("large".into()))));
    assert!(handle.send(set_config("rc2", "mode", ConfigValue::Id("plan".into()))));
    assert_eq!(
        refusal(&mut replies).await,
        ("rc1".to_string(), "config_failed".to_string())
    );
    assert_eq!(
        refusal(&mut replies).await,
        ("rc2".to_string(), "config_failed".to_string())
    );
    assert!(
        began.elapsed() < Duration::from_millis(550),
        "the second switch got a timeout of its own: {:?}",
        began.elapsed()
    );
}
```

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
#[tokio::test]
async fn set_config_reaches_the_actor_and_one_for_a_detached_session_is_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "set-config", configurable_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    let set = |request_id: &str, session_id: &str| CollectorFrame::SetConfig {
        request_id: request_id.into(),
        session_id: session_id.into(),
        config_id: "mode".into(),
        value: hennery_proto::frames::ConfigValue::Id("plan".into()),
    };
    send_frame(&mut sink, &set("r2", "s1")).await;
    let applied = read_until(&mut stream, body_is("s1", "config_applied")).await;
    let HostFrame::Session {
        body: hennery_proto::frames::SessionBody::ConfigApplied { request_id, indexed },
        ..
    } = applied
    else {
        panic!("expected config_applied, got {applied:?}");
    };
    assert_eq!(
        (request_id.as_str(), indexed.current_mode.as_deref()),
        ("r2", Some("plan"))
    );
    send_frame(&mut sink, &set("r3", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r3")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session --test host_connection`
Expected: does not compile: `no variant named 'SetConfig' found for enum 'SessionCmd'`.

- [ ] **Step 3: Implement `set_config` and the live catalogue in the actor**

In `crates/hennery-host/src/session.rs`, replace:

```rust
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigOptionsCapabilities,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, StopReason,
};
```

with:

```rust
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigOptionsCapabilities,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    StopReason,
};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
```

with:

```rust
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::future::Future;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// adapter; the turn's `turn_ended` completes it (ACP core §4.4).
    Cancel { request_id: String, turn_id: String },
}
```

with:

```rust
    /// adapter; the turn's `turn_ended` completes it (ACP core §4.4).
    Cancel { request_id: String, turn_id: String },
    /// Switch one config option (`session/set_config_option`): answered by
    /// `config_applied`, or an error (ACP core §3.3).
    SetConfig {
        request_id: String,
        config_id: String,
        value: ConfigValue,
    },
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
type Reply = Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

struct Turn {
```

with:

```rust
type Reply = Pin<Box<dyn Future<Output = agent_client_protocol::Result<PromptResponse>> + Send>>;

type ConfigReply = Pin<
    Box<
        dyn Future<
                Output = Result<
                    agent_client_protocol::Result<SetSessionConfigOptionResponse>,
                    tokio::time::error::Elapsed,
                >,
            > + Send,
    >,
>;

/// A `set_config` received and not sent yet.
struct QueuedSwitch {
    request_id: String,
    config_id: String,
    value: ConfigValue,
    /// Receipt plus `config_timeout`: past it, the switch is answered
    /// `config_failed`, sent or not.
    deadline: Instant,
}

/// The actor's `set_config` switches. At most one is out at a time: the
/// real adapters handle requests concurrently, so two switches in flight
/// could land in either order (a model switch clamping a mode set after
/// it). The rest wait, in the order they came. Those still waiting or out
/// when the actor ends are answered `not_attached` then, after its last
/// fact: the collector would otherwise wait out its timeout and drop the
/// whole host connection.
struct PendingConfigs {
    uplink: Uplink,
    queued: VecDeque<QueuedSwitch>,
    out: Option<(String, ConfigReply)>,
}

impl Drop for PendingConfigs {
    fn drop(&mut self) {
        let out = self.out.take().map(|(request_id, _)| request_id);
        let queued = self.queued.drain(..).map(|q| q.request_id);
        for request_id in out.into_iter().chain(queued) {
            self.uplink.reply(HostFrame::Error {
                request_id,
                code: "not_attached".into(),
                message: "the session has ended on this host".into(),
            });
        }
    }
}

struct Turn {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id }
                | SessionCmd::Cancel { request_id, .. } => {
                    // Also covers a start that reached this actor while it was
```

with:

```rust
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id }
                | SessionCmd::Cancel { request_id, .. }
                | SessionCmd::SetConfig { request_id, .. } => {
                    // Also covers a start that reached this actor while it was
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            indexed: self.catalogue_extracts(),
        });
        // A load's state updates follow the start they belong to; then the
        // note about what the load dropped (ACP core §4.5), then the one
        // about switches that did not take.
        for payload in replay.kept.iter().cloned() {
            self.emit(update(payload, None));
```

with:

```rust
            indexed: self.catalogue_extracts(),
        });
        // A load's state updates follow the start they belong to, and so do
        // updates the adapter sent while the start's switches ran. They are
        // older than the catalogue just announced, so they carry no
        // catalogue extracts (P-13). Then the note about what the load
        // dropped (ACP core §4.5), then the one about switches that did not
        // take.
        let mut early = replay.kept.clone();
        while let Ok(payload) = updates.try_recv() {
            early.push(payload);
        }
        for payload in early {
            self.emit(update(payload, None));
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        // inside one) is in flight.
        let mut idle_since = Instant::now();
        loop {
```

with:

```rust
        // inside one) is in flight.
        let mut idle_since = Instant::now();
        let mut configs = PendingConfigs {
            uplink: self.uplink.clone(),
            queued: VecDeque::new(),
            out: None,
        };
        loop {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                // the prompt reply it preceded on the wire.
                biased;
                Some(payload) = updates.recv() => self.emit(update(payload, turn.as_ref().map(|t| t.id.as_str()))),
                info = adapter.exited() => {
```

with:

```rust
                // the prompt reply it preceded on the wire.
                biased;
                Some(payload) = updates.recv() => {
                    self.emit(self.live_update(payload, turn.as_ref().map(|t| t.id.as_str())));
                }
                info = adapter.exited() => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                        indexed: self.catalogue_extracts(),
                    }),
                    Some(SessionCmd::Park { .. }) => {
```

with:

```rust
                        indexed: self.catalogue_extracts(),
                    }),
                    // A switch may run during a turn; it is answered in order.
                    Some(SessionCmd::SetConfig { request_id, config_id, value }) => match self.check_switch(&config_id, &value) {
                        Err((code, message)) => self.reject(request_id, code, message),
                        Ok(()) => {
                            let deadline = Instant::now() + self.options.config_timeout;
                            configs.queued.push_back(QueuedSwitch { request_id, config_id, value, deadline });
                            self.send_next_switch(&conn, &agent_session, &mut configs);
                        }
                    },
                    Some(SessionCmd::Park { .. }) => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    }
                }
                _ = cancel_deadline(cancel_at) => {
```

with:

```rust
                    }
                }
                result = next_config(&mut configs) => {
                    let (request_id, _) = configs.out.take().expect("an answer implies a switch");
                    // Updates the adapter sent before its answer come first.
                    self.drain_updates(&mut updates, turn.as_ref().map(|t| t.id.as_str()));
                    self.config_answered(request_id, result);
                    self.send_next_switch(&conn, &agent_session, &mut configs);
                }
                _ = cancel_deadline(cancel_at) => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            message,
        });
    }
```

with:

```rust
            message,
        });
    }

    /// Refuse a `set_config` the adapter could not take: an option it does
    /// not offer (`unknown_option`), or a value of the wrong kind for it
    /// (`invalid`). Whether a select offers the value is the adapter's call.
    fn check_switch(&self, config_id: &str, value: &ConfigValue) -> Result<(), (&'static str, String)> {
        let catalogue = self.catalogue.lock().expect("catalogue lock");
        let Some(option) = catalogue.options.iter().find(|o| &*o.id.0 == config_id) else {
            return Err(("unknown_option", format!("the adapter offers no option {config_id}")));
        };
        match (&option.kind, value) {
            (SessionConfigKind::Select(_), ConfigValue::Id(_))
            | (SessionConfigKind::Boolean(_), ConfigValue::Bool(_)) => Ok(()),
            _ => Err(("invalid", format!("{config_id} takes a different kind of value"))),
        }
    }

    /// Answer a `set_config` from the adapter's answer: `config_applied`
    /// with the catalogue it answered with (none if it answered without
    /// one), or `config_failed`.
    /// Send the oldest waiting switch, unless one is out. A switch whose
    /// deadline passed while it waited is answered `config_failed`.
    fn send_next_switch(&self, conn: &ConnectionTo<Agent>, session: &SessionId, configs: &mut PendingConfigs) {
        while configs.out.is_none()
            && let Some(next) = configs.queued.pop_front()
        {
            if Instant::now() >= next.deadline {
                let message = "an earlier switch is still out".to_string();
                self.reject(next.request_id, "config_failed", message);
                continue;
            }
            let request = SetSessionConfigOptionRequest::new(session.clone(), next.config_id, acp_value(&next.value));
            let reply = tokio::time::timeout_at(next.deadline, conn.send_request(request).block_task());
            configs.out = Some((next.request_id, Box::pin(reply)));
        }
    }

    fn config_answered(
        &self,
        request_id: String,
        result: Result<agent_client_protocol::Result<SetSessionConfigOptionResponse>, tokio::time::error::Elapsed>,
    ) {
        match result {
            Ok(Ok(response)) => {
                {
                    let mut catalogue = self.catalogue.lock().expect("catalogue lock");
                    if response.config_options.is_empty() {
                        catalogue.current = false;
                    } else {
                        *catalogue = Catalogue {
                            options: response.config_options,
                            current: true,
                        };
                    }
                }
                self.emit(SessionBody::ConfigApplied {
                    request_id,
                    indexed: self.catalogue_extracts(),
                });
            }
            Ok(Err(err)) => self.reject(request_id, "config_failed", err.to_string()),
            Err(_) => {
                self.catalogue.lock().expect("catalogue lock").current = false;
                let timeout = self.options.config_timeout;
                let message = format!("no answer within {timeout:?} of the request");
                self.reject(request_id, "config_failed", message);
            }
        }
    }

    /// A live adapter notification as a session frame. A
    /// `config_option_update` (the agent changed its own config, e.g. left
    /// plan mode) replaces the catalogue and carries its extracts.
    fn live_update(&self, payload: Value, turn: Option<&str>) -> SessionBody {
        let mut body = update(payload, turn);
        if let SessionBody::AcpUpdate { indexed, payload } = &mut body
            && let Some(options) = config_update(payload)
        {
            *self.catalogue.lock().expect("catalogue lock") = Catalogue { options, current: true };
            let catalogue = self.catalogue_extracts();
            indexed.config_options = catalogue.config_options;
            indexed.current_model = catalogue.current_model;
            indexed.current_mode = catalogue.current_mode;
            indexed.current_axes = catalogue.current_axes;
        }
        body
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    fn drain_updates(&self, updates: &mut mpsc::UnboundedReceiver<Value>, turn: Option<&str>) {
        while let Ok(payload) = updates.try_recv() {
            self.emit(update(payload, turn));
        }
```

with:

```rust
    fn drain_updates(&self, updates: &mut mpsc::UnboundedReceiver<Value>, turn: Option<&str>) {
        while let Ok(payload) = updates.try_recv() {
            self.emit(self.live_update(payload, turn));
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        // Output the adapter wrote before dying is still in the pipe.
        while let Ok(Some(payload)) = tokio::time::timeout(DRAIN_QUIET, updates.recv()).await {
            self.emit(update(payload, turn.as_ref().map(|t| t.id.as_str())));
        }
```

with:

```rust
        // Output the adapter wrote before dying is still in the pipe.
        while let Ok(Some(payload)) = tokio::time::timeout(DRAIN_QUIET, updates.recv()).await {
            self.emit(self.live_update(payload, turn.as_ref().map(|t| t.id.as_str())));
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
```

with:

```rust
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The answer to the `set_config` that is out, or never if none is.
async fn next_config(
    configs: &mut PendingConfigs,
) -> Result<agent_client_protocol::Result<SetSessionConfigOptionResponse>, tokio::time::error::Elapsed> {
    match configs.out.as_mut() {
        Some((_, reply)) => reply.await,
        None => std::future::pending().await,
```

- [ ] **Step 4: Dispatch `set_config` on the connection**

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::SetConfig { request_id, .. } => uplink.reply(HostFrame::Error {
            request_id,
            code: "invalid".into(),
            message: "this host does not support set_config yet".into(),
        }),
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

with:

```rust
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::SetConfig {
            request_id,
            session_id,
            config_id,
            value,
        } => match live_session(sessions, &session_id) {
            Some(handle)
                if handle.send(SessionCmd::SetConfig {
                    request_id: request_id.clone(),
                    config_id,
                    value,
                }) => {}
            _ => not_attached(uplink, request_id),
        },
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test host_session --test host_connection`
Expected: `host_session` 45 pass, `host_connection` 17 pass.

- [ ] **Step 6: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-host crates/hennery-testkit
git commit -m "feat(host): set_config and the agent's own config changes"
```

Expected: 225 tests pass.

---

### Task 6: Collector store: the catalogue and the stored config

**Files:**
- Modify: `crates/hennery-sessions/src/store.rs`, `crates/hennery-sessions/src/api.rs`
- Test: `crates/hennery-sessions/tests/store.rs`, `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: `Indexed::current_config`, `SessionCatalog` (Task 2).
- Produces:
  - migration 5: `sessions.model`, `sessions.mode`, `sessions.config_axes` (JSON), and `session_catalog(session_id PK, config_options JSON, updated_at)`;
  - `SessionRow.config: SessionConfig`;
  - `ResumeRequest::Starting { events, agent_session_id, committed_seq, config: SessionConfig }`;
  - `Store::catalog(&self, session_id: &str) -> Result<Option<SessionCatalog>>`: `None` for an unknown session, an empty catalogue for one with none.
- Produces (ingest):
  - an applied `session_started`, a `config_applied` for an attached (or presumed-parked) session, and an applied `acp_update` store their snapshot;
  - extracts without one change nothing;
  - a `config_applied` for a detached session is stored unapplied.
- Produces (`api.rs`): `POST …/resume` sends the stored config.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
             ALTER TABLE events DROP COLUMN applied;
             ALTER TABLE sessions DROP COLUMN presumed_parked;
             PRAGMA user_version = 1;",
```

with:

```rust
             ALTER TABLE events DROP COLUMN applied;
             ALTER TABLE sessions DROP COLUMN presumed_parked;
             ALTER TABLE sessions DROP COLUMN model;
             ALTER TABLE sessions DROP COLUMN mode;
             ALTER TABLE sessions DROP COLUMN config_axes;
             DROP TABLE session_catalog;
             PRAGMA user_version = 1;",
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert!(!store.session("s1").unwrap().unwrap().close_requested);
    // Rows written before migration 3 count as applied.
```

with:

```rust
    assert_eq!(store.turn_state("t1").unwrap().as_deref(), Some("started"));
    assert!(!store.session("s1").unwrap().unwrap().close_requested);
    assert!(store.session("s1").unwrap().unwrap().config.is_empty());
    // Rows written before migration 3 count as applied.
```

Append to `crates/hennery-sessions/tests/store.rs`:

```rust
// Plan B2b: the catalogue and the stored config (ACP core §3.2, §8).

use hennery_proto::frames::{ConfigValue, SessionConfig};

/// Catalogue extracts reporting `model` and `mode`, with one other axis.
fn catalogue(model: &str, mode: &str) -> Indexed {
    Indexed {
        config_options: Some(vec![
            json!({"id": "model", "currentValue": model}),
            json!({"id": "mode"}),
        ]),
        current_model: Some(model.into()),
        current_mode: Some(mode.into()),
        current_axes: Some([("fast".to_string(), ConfigValue::Bool(true))].into_iter().collect()),
        ..Indexed::default()
    }
}

fn config(model: &str, mode: &str) -> SessionConfig {
    SessionConfig {
        model: Some(model.into()),
        mode: Some(mode.into()),
        axes: [("fast".to_string(), ConfigValue::Bool(true))].into_iter().collect(),
    }
}

fn started_with(store: &Store, indexed: Indexed) {
    store.create_session("s1", "h1", "fake", "/tmp").unwrap();
    store
        .ingest(
            "s1",
            1,
            &SessionBody::SessionStarted {
                request_id: "r0".into(),
                agent_session_id: "a1".into(),
                indexed,
            },
        )
        .unwrap();
}

fn applied(indexed: Indexed) -> SessionBody {
    SessionBody::ConfigApplied {
        request_id: "rc".into(),
        indexed,
    }
}

fn stored(store: &Store) -> SessionConfig {
    store.session("s1").unwrap().unwrap().config
}

#[test]
fn session_started_stores_the_announced_catalogue_and_its_current_values() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("large", "plan"));
    assert_eq!(stored(&store), config("large", "plan"));
    let catalog = store.catalog("s1").unwrap().unwrap();
    assert_eq!(catalog.config_options.len(), 2);
    assert_eq!(catalog.current, config("large", "plan"));
    assert_eq!(store.catalog("nope").unwrap(), None);
}

#[test]
fn a_session_whose_host_reported_no_catalogue_has_an_empty_one() {
    let store = Store::open_in_memory().unwrap();
    started(&store);
    let catalog = store.catalog("s1").unwrap().unwrap();
    assert!(
        catalog.config_options.is_empty() && catalog.current.is_empty(),
        "{catalog:?}"
    );
}

#[test]
fn a_config_applied_or_a_live_update_replaces_the_catalogue() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("small", "default"));
    let events = store.ingest("s1", 2, &applied(catalogue("large", "default"))).unwrap();
    assert_eq!(kinds(&events), ["config_applied"]);
    assert_eq!(stored(&store), config("large", "default"));
    let live = SessionBody::AcpUpdate {
        indexed: catalogue("large", "bypass"),
        payload: json!({"update": {"sessionUpdate": "config_option_update"}}),
    };
    store.ingest("s1", 3, &live).unwrap();
    assert_eq!(stored(&store), config("large", "bypass"));
    // An update with no catalogue changes nothing.
    store.ingest("s1", 4, &update(1)).unwrap();
    assert_eq!(stored(&store), config("large", "bypass"));
}

/// An adapter's unparseable or empty answer is no read-back: the stored
/// model and mode must survive it, or the next resume would re-apply
/// nothing (P-13 through another door).
#[test]
fn an_empty_or_absent_read_back_keeps_the_stored_catalogue() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("large", "plan"));
    let empty = Indexed {
        config_options: Some(vec![]),
        ..Indexed::default()
    };
    let events = store.ingest("s1", 2, &applied(empty)).unwrap();
    assert_eq!(
        kinds(&events),
        ["config_applied"],
        "still listed: the switch was accepted"
    );
    store.ingest("s1", 3, &applied(Indexed::default())).unwrap();
    assert_eq!(stored(&store), config("large", "plan"));
    assert_eq!(store.catalog("s1").unwrap().unwrap().config_options.len(), 2);
}

#[test]
fn a_late_config_applied_for_a_detached_session_is_not_applied() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("small", "default"));
    parked(&store, 2);
    let events = store.ingest("s1", 3, &applied(catalogue("large", "plan"))).unwrap();
    assert!(events.is_empty(), "{events:?}");
    assert_eq!(stored(&store), config("small", "default"));
    assert!(!kinds(&store.events("s1", 0, 100).unwrap()).contains(&"config_applied"));
}

#[test]
fn a_resume_hands_back_the_config_to_re_apply() {
    let store = Store::open_in_memory().unwrap();
    started_with(&store, catalogue("large", "plan"));
    parked(&store, 2);
    let ResumeRequest::Starting { config: wanted, .. } = store.request_resume("s1").unwrap() else {
        panic!("not resumable");
    };
    assert_eq!(wanted, config("large", "plan"));
}
```

Append to `crates/hennery-testkit/tests/reconcile.rs`:

```rust
// Plan B2b: the collector's side of model, axes and mode.

/// Catalogue extracts whose current mode is `mode`.
fn catalogue(mode: &str) -> hennery_proto::frames::Indexed {
    hennery_proto::frames::Indexed {
        config_options: Some(vec![json!({"id": "mode", "currentValue": mode})]),
        current_mode: Some(mode.into()),
        current_axes: Some(Default::default()),
        ..Default::default()
    }
}

/// The start request's config reaches the host; what the host then reports
/// as current is what a resume re-applies, not what was asked for.
#[tokio::test]
async fn a_resume_re_sends_the_config_its_host_last_reported() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client();
    let url = collector.url("/api/sessions");
    let call = tokio::spawn(async move {
        post(
            &c,
            url,
            json!({ "host_id": HOST, "agent": "fake", "cwd": "/tmp", "mode": "plan", "axes": {"fast": true} }),
        )
        .await
    });
    let CollectorFrame::StartSession {
        request_id,
        session_id,
        config,
        ..
    } = host.next().await
    else {
        panic!("expected start_session");
    };
    assert_eq!(config.mode.as_deref(), Some("plan"));
    assert_eq!(
        config.axes.get("fast"),
        Some(&hennery_proto::frames::ConfigValue::Bool(true))
    );
    // The adapter clamped the mode to `default`.
    host.emit(
        &session_id,
        SessionBody::SessionStarted {
            request_id,
            agent_session_id: "agent-1".into(),
            indexed: catalogue("default"),
        },
    )
    .await;
    assert_eq!(call.await.unwrap().0, 202);
    host.emit(
        &session_id,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        },
    )
    .await;
    wait_for("parked", || async {
        (collector.lifecycle(&session_id) == "parked").then_some(())
    })
    .await;
    let c = client();
    let url = resume_url(&collector, &session_id);
    tokio::spawn(async move { post(&c, url, json!({})).await });
    match host.next().await {
        CollectorFrame::ResumeSession { config, .. } => {
            assert_eq!(config.mode.as_deref(), Some("default"));
            assert!(config.axes.is_empty(), "{config:?}");
        }
        other => panic!("expected resume_session, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-sessions --test store`
Expected: does not compile: `no field 'config' on type 'SessionRow'`, `no method named 'catalog'`, `ResumeRequest::Starting` has no field `config`.

- [ ] **Step 3: Implement the store**

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

use anyhow::Result;
use hennery_proto::frames::{AttachedSession, SessionBody, TurnOutcome};
use hennery_proto::rest::EventDto;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
```

with:

```rust

use anyhow::Result;
use hennery_proto::frames::{AttachedSession, ConfigValue, Indexed, SessionBody, SessionConfig, TurnOutcome};
use hennery_proto::rest::{EventDto, SessionCatalog};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    ALTER TABLE sessions ADD COLUMN presumed_parked INTEGER NOT NULL DEFAULT 0;
",
];
```

with:

```rust
    ALTER TABLE sessions ADD COLUMN presumed_parked INTEGER NOT NULL DEFAULT 0;
",
    // Model, axes and mode (ACP core §8): the current values from the last
    // catalogue a host reported, which a resume re-applies, and the
    // catalogue itself, off the session list (P-23).
    "
    ALTER TABLE sessions ADD COLUMN model TEXT;
    ALTER TABLE sessions ADD COLUMN mode TEXT;
    ALTER TABLE sessions ADD COLUMN config_axes TEXT;
    CREATE TABLE session_catalog (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        config_options TEXT NOT NULL,
        updated_at TEXT NOT NULL);
",
];
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    /// (ACP core §5.3).
    pub presumed_parked: bool,
}
```

with:

```rust
    /// (ACP core §5.3).
    pub presumed_parked: bool,
    /// The model, mode and other axes the host last reported as current;
    /// a resume re-applies them (ACP core §4.3).
    pub config: SessionConfig,
}
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        agent_session_id: String,
        committed_seq: u64,
    },
```

with:

```rust
        agent_session_id: String,
        committed_seq: u64,
        /// The stored config, re-applied by the host after the load.
        config: SessionConfig,
    },
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
}

/// Keep a stored host fact that did not apply as the idempotency key only:
```

with:

```rust
}

/// Store the catalogue snapshot a fact's extracts carry (ACP core §3.2,
/// §8): the options for `GET …/catalog`, and the current values in the
/// session's `model`, `mode` and `config_axes`, which a resume re-applies.
/// Extracts without a snapshot (none, or an empty read-back) change
/// nothing: the stored values are never replaced by a guess (P-13).
fn store_catalogue(tx: &Transaction<'_>, session_id: &str, indexed: &Indexed, ts: &str) -> Result<()> {
    let Some(current) = indexed.current_config() else {
        return Ok(());
    };
    let options = indexed.config_options.clone().unwrap_or_default();
    tx.execute(
        "UPDATE sessions SET model = ?2, mode = ?3, config_axes = ?4 WHERE id = ?1",
        params![
            session_id,
            current.model,
            current.mode,
            serde_json::to_string(&current.axes)?
        ],
    )?;
    tx.execute(
        "INSERT INTO session_catalog(session_id, config_options, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(session_id) DO UPDATE SET config_options = excluded.config_options, updated_at = excluded.updated_at",
        params![session_id, serde_json::to_string(&options)?, ts],
    )?;
    Ok(())
}

/// A session's `model`, `mode` and `config_axes` columns.
type ConfigColumns = (Option<String>, Option<String>, Option<String>);

/// A session's stored config, from its `model`, `mode` and `config_axes`.
fn stored_config((model, mode, axes): ConfigColumns) -> Result<SessionConfig> {
    let axes: BTreeMap<String, ConfigValue> = match axes {
        Some(axes) => serde_json::from_str(&axes)?,
        None => BTreeMap::new(),
    };
    Ok(SessionConfig { model, mode, axes })
}

/// Keep a stored host fact that did not apply as the idempotency key only:
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, host_id, agent, cwd, lifecycle, activity, open_turn_id, failure_reason, close_requested,
                        presumed_parked
                 FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
```

with:

```rust

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        let row = self
            .conn()
            .query_row(
                "SELECT id, host_id, agent, cwd, lifecycle, activity, open_turn_id, failure_reason, close_requested,
                        presumed_parked, model, mode, config_axes
                 FROM sessions WHERE id = ?1",
                [id],
                |r| {
                    let config: ConfigColumns = (r.get(10)?, r.get(11)?, r.get(12)?);
                    let row = SessionRow {
                        id: r.get(0)?,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                        close_requested: r.get(8)?,
                        presumed_parked: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }
```

with:

```rust
                        close_requested: r.get(8)?,
                        presumed_parked: r.get(9)?,
                        config: SessionConfig::default(),
                    };
                    Ok((row, config))
                },
            )
            .optional()?;
        let Some((mut row, config)) = row else {
            return Ok(None);
        };
        row.config = stored_config(config)?;
        Ok(Some(row))
    }

    /// The session's config catalogue and current values (ACP core §9);
    /// `None` for an unknown session, an empty catalogue for one whose
    /// host has reported none.
    pub fn catalog(&self, session_id: &str) -> Result<Option<SessionCatalog>> {
        let row: Option<(ConfigColumns, Option<String>)> = self
            .conn()
            .query_row(
                "SELECT s.model, s.mode, s.config_axes, c.config_options
                 FROM sessions s LEFT JOIN session_catalog c ON c.session_id = s.id WHERE s.id = ?1",
                [session_id],
                |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get(3)?)),
            )
            .optional()?;
        let Some((config, options)) = row else {
            return Ok(None);
        };
        Ok(Some(SessionCatalog {
            session_id: session_id.to_string(),
            config_options: match options {
                Some(options) => serde_json::from_str(&options)?,
                None => Vec::new(),
            },
            current: stored_config(config)?,
        }))
    }
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT lifecycle, agent_session_id, open_turn_id FROM sessions WHERE id = ?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((lifecycle, agent_session_id, open_turn)) = row else {
            return Ok(ResumeRequest::NotFound);
```

with:

```rust
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, Option<String>, Option<String>, ConfigColumns)> = tx
            .query_row(
                "SELECT lifecycle, agent_session_id, open_turn_id, model, mode, config_axes FROM sessions WHERE id = ?1",
                [session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, (r.get(3)?, r.get(4)?, r.get(5)?))),
            )
            .optional()?;
        let Some((lifecycle, agent_session_id, open_turn, config)) = row else {
            return Ok(ResumeRequest::NotFound);
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            agent_session_id,
            committed_seq: committed.unwrap_or(0) as u64,
        })
```

with:

```rust
            agent_session_id,
            committed_seq: committed.unwrap_or(0) as u64,
            config: stored_config(config)?,
        })
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        }];
        match body {
            SessionBody::SessionStarted { agent_session_id, .. } => {
                // A re-emitted `session_started` for a session already
```

with:

```rust
        }];
        match body {
            SessionBody::SessionStarted {
                agent_session_id,
                indexed,
                ..
            } => {
                // A re-emitted `session_started` for a session already
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::StartFailed { code, .. } => {
```

with:

```rust
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    // The catalogue after the start's switches (P-13).
                    store_catalogue(&tx, session_id, indexed, &ts)?;
                }
            }
            SessionBody::StartFailed { code, .. } => {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                if !fact_applies(&tx, session_id, indexed.turn_id.as_deref())? {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
```

with:

```rust
                if !fact_applies(&tx, session_id, indexed.turn_id.as_deref())? {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    // A live `config_option_update` (the agent changed its
                    // own config); the host never sends a replayed one
                    // with extracts.
                    store_catalogue(&tx, session_id, indexed, &ts)?;
                }
            }
            SessionBody::ConfigApplied { indexed, .. } => {
                // The read-back of a switch on an attached session. A late
                // one for a session that has detached since changes nothing.
                let (lifecycle, presumed): (String, bool) = tx.query_row(
                    "SELECT lifecycle, presumed_parked FROM sessions WHERE id = ?1",
                    [session_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                if lifecycle == "active" || presumed {
                    store_catalogue(&tx, session_id, indexed, &ts)?;
                } else {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing; nor does a `config_applied` until the
            // catalogue is stored.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } | SessionBody::ConfigApplied { .. } => {
                if !fact_applies(&tx, session_id, None)? {
```

with:

```rust
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } => {
                if !fact_applies(&tx, session_id, None)? {
```

- [ ] **Step 4: Re-send the stored config on resume**

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, SessionBody, SessionConfig};
use hennery_proto::rest::{
```

with:

```rust
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, SessionBody};
use hennery_proto::rest::{
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        return request_failed(RequestError::NotConnected);
    }
    let (agent_session_id, committed_seq) = match state.store.request_resume(&id) {
        Ok(ResumeRequest::Starting {
```

with:

```rust
        return request_failed(RequestError::NotConnected);
    }
    let (agent_session_id, committed_seq, config) = match state.store.request_resume(&id) {
        Ok(ResumeRequest::Starting {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
            agent_session_id,
            committed_seq,
        }) => {
```

with:

```rust
            agent_session_id,
            committed_seq,
            config,
        }) => {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
                state.hub.publish(event);
            }
            (agent_session_id, committed_seq)
        }
```

with:

```rust
                state.hub.publish(event);
            }
            (agent_session_id, committed_seq, config)
        }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        cwd: session.cwd,
        agent_session_id,
        config: SessionConfig::default(),
    };
```

with:

```rust
        cwd: session.cwd,
        agent_session_id,
        // Re-applied after the load (ACP core §4.3).
        config,
    };
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-sessions --test store && cargo test -p hennery-testkit --test reconcile`
Expected: `store` 57 pass (the rolled-back migration test now also drops migration 5's columns and table), `reconcile` 29 pass.

- [ ] **Step 6: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-sessions crates/hennery-testkit
git commit -m "feat(sessions): store the catalogue from extracts and re-apply it on resume"
```

Expected: 232 tests pass.

---

### Task 7: Collector API: `POST …/config`, `GET …/catalog`, SSE `catalog_changed`

**Files:**
- Modify: `crates/hennery-sessions/src/api.rs`, `crates/hennery-sessions/src/ws.rs`
- Test: `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: `Store::catalog`, the catalogue on ingest (Task 6); `CollectorFrame::SetConfig`, `ConfigRequest`, `SessionCatalog::from_indexed` (Task 2).
- Produces:
  - `POST /api/sessions/{id}/config` (`ConfigRequest`) → 202 `SessionCatalog`, with the refusals of decision 7;
  - `GET /api/sessions/{id}/catalog` → 200 `SessionCatalog` | 404;
  - on `GET /api/stream/sessions/{id}`, a `catalog_changed` message (same `id:`, data `SessionCatalog`) after each event that carries a snapshot, live and on replay;
  - `ws.rs` resolves a `set_config` waiter by the `config_applied` that carries its request id;
  - `CONFIG_TIMEOUT` (60 s) in the timeout assertion; `unknown_option` maps to 409.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/reconcile.rs`:

```rust
fn config_url(collector: &Collector, session: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/config"))
}

/// The next frame must be a `set_config` for `session`; returns its request
/// id, config id and value.
async fn expect_set_config(host: &mut ScriptedHost, session: &str) -> (String, String, Value) {
    match host.next().await {
        CollectorFrame::SetConfig {
            request_id,
            session_id,
            config_id,
            value,
        } => {
            assert_eq!(session_id, session);
            (request_id, config_id, serde_json::to_value(value).unwrap())
        }
        other => panic!("expected set_config, got {other:?}"),
    }
}

#[tokio::test]
async fn set_config_answers_with_the_catalogue_the_host_read_back() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client();
    let url = config_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "mode", "value": "plan" })).await });
    let (request_id, config_id, value) = expect_set_config(&mut host, &session).await;
    assert_eq!((config_id.as_str(), value), ("mode", json!("plan")));
    host.emit(
        &session,
        SessionBody::ConfigApplied {
            request_id,
            indexed: catalogue("plan"),
        },
    )
    .await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["mode"].as_str()), (202, Some("plan")), "{body}");
    assert_eq!(body["config_options"], json!([{"id": "mode", "currentValue": "plan"}]));
    let (status, catalog) = get(&client(), collector.url(&format!("/api/sessions/{session}/catalog"))).await;
    assert_eq!((status, catalog), (200, body));
}

#[tokio::test]
async fn set_config_refusals_answer_with_their_codes() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    for (code, status) in [("unknown_option", 409), ("config_failed", 502), ("invalid", 400)] {
        let c = client();
        let url = config_url(&collector, &session);
        let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "model", "value": "huge" })).await });
        let (request_id, _, _) = expect_set_config(&mut host, &session).await;
        host.send(&HostFrame::Error {
            request_id,
            code: code.into(),
            message: "no".into(),
        })
        .await;
        let (got, body) = call.await.unwrap();
        assert_eq!((got, body["code"].as_str()), (status, Some(code)), "{body}");
    }
    // Not a string or a boolean: refused before anything is sent.
    let (status, _) = post(
        &client(),
        config_url(&collector, &session),
        json!({ "config_id": "x", "value": 3 }),
    )
    .await;
    assert_eq!(status, 422);
    let (status, _) = post(
        &client(),
        config_url(&collector, "nope"),
        json!({ "config_id": "x", "value": "y" }),
    )
    .await;
    assert_eq!(status, 404);
    host.emit(
        &session,
        SessionBody::SessionParked {
            reason: hennery_proto::frames::ParkReason::Idle,
        },
    )
    .await;
    wait_for("parked", || async {
        (collector.lifecycle(&session) == "parked").then_some(())
    })
    .await;
    let (status, body) = post(
        &client(),
        config_url(&collector, &session),
        json!({ "config_id": "x", "value": "y" }),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
    let more = tokio::time::timeout(Duration::from_millis(300), host.next()).await;
    assert!(more.is_err(), "a request reached the host: {more:?}");
}

/// Read a session's SSE stream from its start until `pred` holds for the
/// text received so far.
async fn read_stream(collector: &Collector, session: &str, pred: impl Fn(&str) -> bool) -> String {
    use futures::StreamExt;
    let resp = client()
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
        .send()
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    let mut buf = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !pred(&buf) {
        let chunk = tokio::time::timeout_at(deadline, body.next())
            .await
            .unwrap_or_else(|_| panic!("stream stalled: {buf}"))
            .unwrap()
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(&chunk));
    }
    buf
}

#[tokio::test]
async fn every_catalogue_change_is_also_a_catalog_changed_message_on_the_session_stream() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    // The agent changes its own mode: a live update with the catalogue.
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: catalogue("bypass"),
            payload: json!({"update": {"sessionUpdate": "config_option_update"}}),
        },
    )
    .await;
    // An update without one is only an event.
    host.emit(
        &session,
        SessionBody::AcpUpdate {
            indexed: Default::default(),
            payload: json!({"update": {"sessionUpdate": "agent_message_chunk"}}),
        },
    )
    .await;
    let stream = read_stream(&collector, &session, |s| s.matches("event: event").count() >= 3).await;
    let changed: Vec<&str> = stream
        .split("\n\n")
        .filter(|m| m.contains("event: catalog_changed"))
        .collect();
    assert_eq!(changed.len(), 1, "{stream}");
    assert!(changed[0].contains(r#""mode":"bypass""#), "{}", changed[0]);
    let id = |m: &str| m.lines().find(|l| l.starts_with("id: ")).map(str::to_string);
    let update = stream
        .split("\n\n")
        .find(|m| m.contains("config_option_update"))
        .unwrap();
    assert_eq!(id(changed[0]), id(update), "catalog_changed carries its event's id");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p hennery-testkit --test reconcile -- config catalog`
Expected: `a_resume_re_sends_the_config_its_host_last_reported` (Task 6) passes; the two `set_config_…` tests FAIL (`a collector frame within 10s`: no route sends `set_config`), and `every_catalogue_change_…` FAILS (`stream stalled`: no `catalog_changed` is sent).

- [ ] **Step 3: Resolve `config_applied` in the socket task**

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
                        match &body {
                            SessionBody::SessionStarted { request_id, .. }
                            | SessionBody::TurnStarted { request_id, .. } => {
                                state.hub.resolve(request_id, body.clone());
```

with:

```rust
                        match &body {
                            SessionBody::SessionStarted { request_id, .. }
                            | SessionBody::TurnStarted { request_id, .. }
                            | SessionBody::ConfigApplied { request_id, .. } => {
                                state.hub.resolve(request_id, body.clone());
```

- [ ] **Step 4: Add the routes and the SSE message**

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, SessionBody};
use hennery_proto::rest::{
    ApiError, CancelResponse, EventDto, LifecycleResponse, OpenTurn, PromptRequest, PromptResponse, SessionDetail,
    StartSessionRequest, StartSessionResponse,
};
```

with:

```rust
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, Indexed, SessionBody};
use hennery_proto::rest::{
    ApiError, CancelResponse, ConfigRequest, EventDto, LifecycleResponse, OpenTurn, PromptRequest, PromptResponse,
    SessionCatalog, SessionDetail, StartSessionRequest, StartSessionResponse,
};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// cancel well before this (`hennery_host::session::CANCEL_GRACE`).
const CANCEL_TIMEOUT: Duration = Duration::from_secs(60);

```

with:

```rust
/// cancel well before this (`hennery_host::session::CANCEL_GRACE`).
const CANCEL_TIMEOUT: Duration = Duration::from_secs(60);
/// `set_config` (ACP core §3.4). The host answers a switch the adapter
/// leaves hanging well before this (`hennery_host::session::CONFIG_TIMEOUT`).
const CONFIG_TIMEOUT: Duration = Duration::from_secs(60);

```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        && PROMPT_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && TEARDOWN_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && CANCEL_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
    "every request timeout must exceed the host connection's read deadline"
```

with:

```rust
        && PROMPT_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && TEARDOWN_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && CANCEL_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis()
        && CONFIG_TIMEOUT.as_millis() > crate::ws::READ_TIMEOUT.as_millis(),
    "every request timeout must exceed the host connection's read deadline"
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .route("/api/sessions/{id}/park", post(park))
        .route("/api/sessions/{id}/close", post(close))
        .route("/api/sessions/{id}/events", get(events))
```

with:

```rust
        .route("/api/sessions/{id}/park", post(park))
        .route("/api/sessions/{id}/close", post(close))
        .route("/api/sessions/{id}/catalog", get(catalog))
        .route("/api/sessions/{id}/config", post(set_config))
        .route("/api/sessions/{id}/events", get(events))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        RequestError::Rejected { code, message } => {
            let status = match code.as_str() {
                "not_attached" | "turn_in_progress" | "not_running" => StatusCode::CONFLICT,
                "unknown_agent" | "start_failed" => StatusCode::BAD_GATEWAY,
```

with:

```rust
        RequestError::Rejected { code, message } => {
            let status = match code.as_str() {
                "not_attached" | "turn_in_progress" | "not_running" | "unknown_option" => StatusCode::CONFLICT,
                "unknown_agent" | "start_failed" => StatusCode::BAD_GATEWAY,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

with:

```rust
}

/// The session's config catalogue and current values (ACP core §9).
async fn catalog(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    catalog_response(&state, &id, StatusCode::OK)
}

fn catalog_response(state: &AppState, id: &str, status: StatusCode) -> Response {
    match state.store.catalog(id) {
        Ok(Some(catalog)) => (status, Json(catalog)).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}

/// Switch one config option of an attached session (ACP core §9): 202
/// with the session's catalogue once the host's `config_applied` is
/// ingested. Every viewer sees the change as SSE `catalog_changed`.
async fn set_config(State(state): State<AppState>, Path(id): Path<String>, Json(req): Json<ConfigRequest>) -> Response {
    let session = match state.store.session(&id) {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    };
    if session.lifecycle != "active" || !state.hub.is_ready(&session.host_id) {
        return error(StatusCode::CONFLICT, "not_attached", "resume the session first");
    }
    let request_id = uuid::Uuid::now_v7().to_string();
    let frame = CollectorFrame::SetConfig {
        request_id: request_id.clone(),
        session_id: id.clone(),
        config_id: req.config_id,
        value: req.value,
    };
    match state
        .hub
        .request(&session.host_id, &request_id, frame, CONFIG_TIMEOUT)
        .await
    {
        Ok(_) => catalog_response(&state, &id, StatusCode::ACCEPTED),
        Err(err) => request_failed(err),
    }
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
}

/// Session stream: replays from `Last-Event-ID`, then follows live events.
```

with:

```rust
}

/// The SSE messages for one stored event: the event, then `catalog_changed`
/// with the same id if it carries a catalogue snapshot (ACP core §9). A
/// listed event with a snapshot is one that changed the stored catalogue,
/// and both come from the stored row, so a replay from `Last-Event-ID`
/// sends them too.
fn sse_messages(e: &EventDto) -> Vec<Result<Event, Infallible>> {
    let mut out = vec![Ok(sse_event(e))];
    if let Some(catalog) = catalog_in(e) {
        out.push(Ok(Event::default()
            .id(e.event_id.to_string())
            .event("catalog_changed")
            .data(serde_json::to_string(&catalog).expect("catalog serializes"))));
    }
    out
}

/// The catalogue snapshot a stored host fact carries in its extracts.
fn catalog_in(e: &EventDto) -> Option<SessionCatalog> {
    if !matches!(e.kind.as_str(), "session_started" | "config_applied" | "acp_update") {
        return None;
    }
    let indexed: Indexed = serde_json::from_value(e.body.get("indexed")?.clone()).ok()?;
    SessionCatalog::from_indexed(&e.session_id, &indexed)
}

/// Session stream: replays from `Last-Event-ID`, then follows live events.
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    let backlog = state.store.events(&id, after, u32::MAX).unwrap_or_default();
    let last = backlog.last().map(|e| e.event_id).unwrap_or(after);
    let replay = stream::iter(backlog.into_iter().map(|e| Ok(sse_event(&e))));
    let session = id.clone();
    let follow = live.filter_map(move |item| {
        let session = session.clone();
        async move {
            match item {
                Ok(e) if e.session_id == session && e.event_id > last => Some(Ok(sse_event(&e))),
                Ok(_) => None,
                // Lagged: tell the client to refetch instead of skipping silently.
                Err(_) => Some(Ok(Event::default().event("resync_required").data("{}"))),
            }
        }
    });
    let stream = replay
```

with:

```rust
    let backlog = state.store.events(&id, after, u32::MAX).unwrap_or_default();
    let last = backlog.last().map(|e| e.event_id).unwrap_or(after);
    let replay = stream::iter(backlog.iter().flat_map(sse_messages).collect::<Vec<_>>());
    let session = id.clone();
    let follow = live
        .filter_map(move |item| {
            let session = session.clone();
            async move {
                match item {
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&e)),
                    Ok(_) => None,
                    // Lagged: tell the client to refetch instead of skipping silently.
                    Err(_) => Some(vec![Ok(Event::default().event("resync_required").data("{}"))]),
                }
            }
        })
        .flat_map(stream::iter);
    let stream = replay
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test reconcile`
Expected: all 32 pass.

- [ ] **Step 6: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-sessions crates/hennery-testkit
git commit -m "feat(sessions): POST config, GET catalog and SSE catalog_changed"
```

Expected: 235 tests pass.

---

### Task 8: End to end: scenario 1 and the config gates against the fake

**Files:**
- Test: `crates/hennery-testkit/tests/e2e.rs`

**Interfaces:**
- Consumes: everything above, over a real collector, a real host and the fake adapter as a child process.

These tests exercise Tasks 2–7 together. They pass on their first run. If one fails, the defect is in an earlier task, and that task's own tests are missing a case: add it there.

- [ ] **Step 1: Write the tests**

Append to `crates/hennery-testkit/tests/e2e.rs`:

```rust
// Plan B2b: model, axes and mode end to end (ACP core §12 scenario 1 and
// the fake-adapter half of its live gates).

fn config_script(log: &Path) -> FakeScript {
    FakeScript {
        config_options: hennery_testkit::sample_config_options(),
        model_switch_sets_mode: Some("default".into()),
        config_log: Some(log.to_string_lossy().into_owned()),
        ..FakeScript::default()
    }
}

async fn catalog(c: &reqwest::Client, collector: &Collector, session: &str) -> Value {
    let url = collector.url(&format!("/api/sessions/{session}/catalog"));
    let resp = c.get(url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

fn current(catalog: &Value) -> (Value, Value, Value) {
    (
        catalog["model"].clone(),
        catalog["mode"].clone(),
        catalog["axes"].clone(),
    )
}

/// Scenario 1: one request starts the session with model, mode and axes;
/// the mode goes last, so the model's clamp cannot undo it, and the
/// announced catalogue is the one after the switches.
#[tokio::test]
async fn a_start_with_model_mode_and_axes_announces_the_catalogue_after_the_switches() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client();
    wait_host_connected(&c, &collector).await;
    let (status, body) = post_json(
        &c,
        collector.url("/api/sessions"),
        json!({
            "host_id": "host-1", "agent": "fake", "cwd": std::env::temp_dir(),
            "model": "large", "mode": "plan", "axes": {"effort": "high"}
        }),
    )
    .await;
    assert_eq!(status, 202, "{body}");
    let session = body["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "model=large\neffort=high\nmode=plan\n"
    );
    let catalog = catalog(&c, &collector, &session).await;
    assert_eq!(
        current(&catalog),
        (json!("large"), json!("plan"), json!({"effort": "high", "fast": false}))
    );
    assert_eq!(catalog["config_options"].as_array().map(Vec::len), Some(4));
}

/// The model-switch gate against the fake: the read-back is what the
/// adapter reports, including the mode it clamped, and a model it refuses
/// is never reported as current.
#[tokio::test]
async fn a_model_switch_answers_with_the_adapters_read_back_and_a_bogus_model_is_never_current() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let url = collector.url(&format!("/api/sessions/{session}/config"));
    let (status, body) = post_json(&c, url.clone(), json!({ "config_id": "mode", "value": "plan" })).await;
    assert_eq!((status, body["mode"].as_str()), (202, Some("plan")), "{body}");
    let (status, body) = post_json(&c, url.clone(), json!({ "config_id": "model", "value": "large" })).await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(
        (body["model"].as_str(), body["mode"].as_str()),
        (Some("large"), Some("default")),
        "the adapter clamped the mode: {body}"
    );
    let (status, body) = post_json(&c, url, json!({ "config_id": "model", "value": "bogus" })).await;
    assert_eq!((status, body["code"].as_str()), (502, Some("config_failed")), "{body}");
    assert_eq!(catalog(&c, &collector, &session).await["model"], "large");
    let evs = events(&c, &collector, &session).await;
    assert_eq!(of_kind(&evs, "config_applied").len(), 2);
}

/// The resume gate against the fake: a mode the agent chose by itself mid
/// turn is stored from its live update, and after a host restart the
/// resume applies it to the new adapter, which starts from its default.
#[tokio::test]
async fn a_mode_the_agent_chose_survives_a_host_restart_and_resume() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("config.log");
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        prompt_sets_mode: Some("bypass".into()),
        ..config_script(&log)
    };
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt_and_wait(&c, &collector, &session, 1).await;
    assert_eq!(catalog(&c, &collector, &session).await["mode"], "bypass");
    assert_eq!(std::fs::read_to_string(&log).unwrap_or_default(), "");

    host.abort();
    let _ = host.await;
    start_host_with(collector.addr, &dir.path().join("host"), fake);
    lifecycle_is(&collector, &session, "parked").await;
    let (status, body) = resume(&c, &collector, &session).await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("active")), "{body}");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "mode=bypass\n");
    let catalog = catalog(&c, &collector, &session).await;
    assert_eq!(current(&catalog).1, json!("bypass"));
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p hennery-testkit --test e2e`
Expected: all 20 pass.

- [ ] **Step 3: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-testkit
git commit -m "test(e2e): model, axes and mode over a real host and adapter"
```

Expected: 238 tests pass.

---

### Task 9: An actor that ends by itself marks its handle ending

**Files:**
- Modify: `crates/hennery-host/src/session.rs`, `crates/hennery-host/src/connection.rs` (comment)
- Test: `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Produces: `SessionHandle::is_ending()` is also true from the moment the actor begins to end by itself: the idle reaper fires, the adapter exits, or the adapter is stopped for ignoring a cancel. It stays true while the actor kills the adapter. `connection.rs` already routes a start or resume for an ending handle to "wait for `finished`, then attach a fresh adapter" (plan B1 decision 5), so it needs no change beyond its comment.

- [ ] **Step 1: Write the failing test**

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
// Plan B2b: an actor that ends by itself says so at once (B2a's hand-off).

async fn wait_ending(handle: &SessionHandle) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !handle.is_ending() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the handle never said it was ending"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An idle reap or an adapter stopped for ignoring a cancel ends the actor
/// by itself. Its handle must say so while the adapter is still being
/// killed (here the whole 1 s grace: it ignores SIGTERM), or a resume
/// arriving then is routed to the ending actor and answered `not_attached`.
#[tokio::test]
async fn an_actor_ending_by_itself_marks_its_handle_ending_while_it_kills_the_adapter() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let reaped = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_ignoring_sigterm(&FakeScript::default()),
        std::env::temp_dir(),
        SessionOptions {
            idle_timeout: Some(Duration::from_millis(300)),
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    wait_ending(&reaped).await;
    assert!(!reaped.is_ended(), "still killing its adapter");
    wait_until(&uplink, has("session_parked:idle")).await;
    wait_ended(&reaped).await;

    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        ignore_cancel: true,
        ..slow_script()
    };
    let stopped = session::spawn(
        uplink.clone(),
        "r0".into(),
        "s2".into(),
        fake_ignoring_sigterm(&script),
        std::env::temp_dir(),
        SessionOptions {
            cancel_grace: Duration::from_millis(300),
            kill_grace: Duration::from_secs(1),
            ..SessionOptions::default()
        },
    );
    wait_until(&uplink, has("session_started")).await;
    assert!(stopped.send(prompt("r1", "t1")));
    wait_until(&uplink, has("turn_started")).await;
    assert!(stopped.send(cancel("rc", "t1")));
    assert!(!stopped.is_ending(), "a cancel alone does not end the actor");
    wait_ending(&stopped).await;
    assert!(!stopped.is_ended(), "still killing its adapter");
    wait_until(&uplink, has("session_parked:operator")).await;
    wait_ended(&stopped).await;
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p hennery-testkit --test host_session ending_by_itself`
Expected: FAILS after 10 s with `the handle never said it was ending`.

- [ ] **Step 3: Implement**

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// Cancelled when the actor's task has finished (its adapter is gone).
    done: CancellationToken,
    /// A park or close has been queued: this actor will serve no further
    /// start or resume, even before it has read that command.
    ending: Arc<AtomicBool>,
```

with:

```rust
    /// Cancelled when the actor's task has finished (its adapter is gone).
    done: CancellationToken,
    /// This actor will serve no further start or resume: a park or close
    /// has been queued (even before the actor has read it), or the actor
    /// has begun ending by itself.
    ending: Arc<AtomicBool>,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    }

    /// A park or close has been queued through `send` (set only there; an
    /// actor that ends by itself — an idle reap, an adapter exit — never
    /// sets it, and shows only as `is_ended` once it is done): a `Restart`
    /// sent now would be answered `not_attached` once the actor gets to it.
    pub fn is_ending(&self) -> bool {
```

with:

```rust
    }

    /// The actor is ending: a park or close has been queued through `send`,
    /// or the actor began ending by itself (an idle reap, an adapter exit, an
    /// adapter stopped for ignoring a cancel) and may still be killing its
    /// adapter. A `Restart` sent now would be answered `not_attached` once
    /// the actor gets to it, so a start or resume waits for `finished` and
    /// attaches a fresh adapter instead.
    pub fn is_ending(&self) -> bool {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    let (tx, rx) = mpsc::unbounded_channel();
    let open_turn = Arc::new(Mutex::new(None));
    let actor = Actor {
```

with:

```rust
    let (tx, rx) = mpsc::unbounded_channel();
    let open_turn = Arc::new(Mutex::new(None));
    let ending = Arc::new(AtomicBool::new(false));
    let actor = Actor {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        options,
        catalogue: Mutex::new(Catalogue::default()),
    };
```

with:

```rust
        options,
        catalogue: Mutex::new(Catalogue::default()),
        ending: ending.clone(),
    };
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        open_turn,
        done,
        ending: Arc::new(AtomicBool::new(false)),
    }
```

with:

```rust
        open_turn,
        done,
        ending,
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The adapter's config options as last reported.
    catalogue: Mutex<Catalogue>,
}
```

with:

```rust
    /// The adapter's config options as last reported.
    catalogue: Mutex<Catalogue>,
    /// Shared with the handle (`SessionHandle::is_ending`).
    ending: Arc<AtomicBool>,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            Indexed::default()
        }
    }
```

with:

```rust
            Indexed::default()
        }
    }

    /// This actor is about to end by itself: from now on a start or resume
    /// waits for it to finish instead of being routed to it.
    fn begin_ending(&self) {
        self.ending.store(true, Ordering::SeqCst);
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                _ = idle_deadline(self.options.idle_timeout, idle_since), if turn.is_none() => {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    self.teardown(&mut adapter, &mut updates, None).await;
```

with:

```rust
                _ = idle_deadline(self.options.idle_timeout, idle_since), if turn.is_none() => {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    self.begin_ending();
                    self.teardown(&mut adapter, &mut updates, None).await;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let grace = self.options.cancel_grace;
        tracing::warn!(session_id = %self.session_id, ?grace, "adapter ignored session/cancel; stopping it");
        self.drain_updates(updates, Some(&turn.id));
```

with:

```rust
        let grace = self.options.cancel_grace;
        tracing::warn!(session_id = %self.session_id, ?grace, "adapter ignored session/cancel; stopping it");
        self.begin_ending();
        self.drain_updates(updates, Some(&turn.id));
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    ) {
        tracing::warn!(session_id = %self.session_id, exit = %describe(info), "adapter exited");
        // Output the adapter wrote before dying is still in the pipe.
```

with:

```rust
    ) {
        tracing::warn!(session_id = %self.session_id, exit = %describe(info), "adapter exited");
        self.begin_ending();
        // Output the adapter wrote before dying is still in the pipe.
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let options = cfg.session_options();
    match live_session(sessions, &req.session_id).filter(SessionHandle::is_ending) {
        // A park or close is queued ahead of this request, so a `Restart`
        // would be answered `not_attached`: attach a fresh adapter once the
        // old one is gone.
        Some(old) => {
```

with:

```rust
    let options = cfg.session_options();
    match live_session(sessions, &req.session_id).filter(SessionHandle::is_ending) {
        // The actor is ending (a park or close is queued ahead of this
        // request, or it is ending by itself), so a `Restart` would be
        // answered `not_attached`: attach a fresh adapter once the old one is
        // gone.
        Some(old) => {
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p hennery-testkit --test host_session`
Expected: all 46 pass.

- [ ] **Step 5: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
git add crates/hennery-host crates/hennery-testkit
git commit -m "fix(host): an actor ending by itself marks its handle ending"
```

Expected: 239 tests pass.

---

### Task 10: A flooding adapter cannot hold off a cancel

**Files:**
- Modify: `crates/hennery-host/src/session.rs`, `crates/hennery-testkit/src/lib.rs`, `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`
- Test: `crates/hennery-testkit/tests/fake_acp.rs`, `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Produces: `FakeScript.flood: bool`. The fake streams `chunks` over and over, back to back, never sleeping, until the prompt is cancelled.
- Produces (behaviour):
  - `UPDATE_BURST` (64): after that many updates in a row, the actor's other arms get a pass;
  - `drain_updates` and the pre-loop drain take only what is queued when they start;
  - a prompt drains what is queued before its `turn_started`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/fake_acp.rs`:

```rust
#[test]
fn flood_streams_until_the_prompt_is_cancelled() {
    let out = exchange_until(r#"{"chunks":["x"],"flood":true}"#, &cancelled_prompt_requests(), 3);
    // It stops for the cancel, however many chunks it got out first.
    assert_eq!(out.last().unwrap()["result"]["stopReason"], "cancelled", "{out:?}");
}
```

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
/// An adapter streaming faster than the outbox can write keeps the actor's
/// update arm always ready. Without a cap on updates in a row, the Cancel
/// behind them is never read, and the turn only ends when the agent does.
/// The outbox is on disk, as on a real host, so the actor is the slow side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_flooding_adapter_cannot_hold_off_a_cancel() {
    let dir = tempfile::tempdir().unwrap();
    let (uplink, _replies) = Uplink::new(Outbox::open(&dir.path().join("outbox.db")).unwrap());
    let script = FakeScript {
        chunks: vec!["flood".into()],
        flood: true,
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while handle.open_turn_id().is_none() {
        assert!(tokio::time::Instant::now() < deadline, "the turn never started");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(handle.send(cancel("rc", "t1")));
    // Polled on the handle: reading a flooded outbox every few ms would
    // itself slow the actor down.
    while handle.open_turn_id().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the cancel was never read: the flood held it off"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ends = turn_ends(&uplink.pending().unwrap());
    assert_eq!(ends, [("t1".to_string(), TurnOutcome::Cancelled, None)]);
}
```

- [ ] **Step 2: Give the fake its flood**

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub config_in_update_only: bool,
}
```

with:

```rust
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub config_in_update_only: bool,
    /// Stream `chunks` over and over, back to back and without sleeping,
    /// until the prompt is cancelled (an adapter flooding the host).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flood: bool,
}
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            sticky_options: Vec::new(),
            config_in_update_only: false,
        }
```

with:

```rust
            sticky_options: Vec::new(),
            config_in_update_only: false,
            flood: false,
        }
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                    }
                    cx.spawn(async move {
                        for (sent, chunk) in script.chunks.into_iter().enumerate() {
```

with:

```rust
                    }
                    cx.spawn(async move {
                        if script.flood {
                            // Back to back until cancelled, yielding (never
                            // sleeping) so the cancel can land.
                            for chunk in script.chunks.iter().cycle() {
                                if *cancelled.borrow() {
                                    return responder.respond(PromptResponse::new(StopReason::Cancelled));
                                }
                                cx2.send_notification(SessionNotification::new(
                                    req.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                                        TextContent::new(chunk.clone()),
                                    ))),
                                ))?;
                                tokio::task::yield_now().await;
                            }
                        }
                        for (sent, chunk) in script.chunks.into_iter().enumerate() {
```

- [ ] **Step 3: Run the tests to verify the host test fails**

Run: `cargo test -p hennery-testkit --test fake_acp --test host_session flood`
Expected: `flood_streams_until_the_prompt_is_cancelled` passes. `a_flooding_adapter_cannot_hold_off_a_cancel` FAILS after 20 s with `the cancel was never read: the flood held it off`. With the outbox on disk the actor is the slow side, so its update arm is always ready and the biased select never reads the Cancel. This failed 8 times in 8 runs while the plan was built.

- [ ] **Step 4: Cap the updates in a row**

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// notification stream has been quiet this long.
const DRAIN_QUIET: Duration = Duration::from_millis(100);

```

with:

```rust
/// notification stream has been quiet this long.
const DRAIN_QUIET: Duration = Duration::from_millis(100);

/// At most this many adapter updates are emitted in a row before the actor's
/// other arms (commands, the prompt's reply, the cancel grace) get a turn:
/// an adapter streaming faster than the outbox writes must not delay a
/// cancel until its turn is over.
const UPDATE_BURST: usize = 64;

```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        // take.
        let mut early = replay.kept.clone();
        while let Ok(payload) = updates.try_recv() {
            early.push(payload);
        }
```

with:

```rust
        // take.
        let mut early = replay.kept.clone();
        for _ in 0..updates.len() {
            match updates.try_recv() {
                Ok(payload) => early.push(payload),
                Err(_) => break,
            }
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            out: None,
        };
        loop {
```

with:

```rust
            out: None,
        };
        // Updates emitted in a row since another arm last had a turn.
        let mut burst = 0;
        loop {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            tokio::select! {
                // Biased: adapter output already received is emitted before
                // the prompt reply it preceded on the wire.
                biased;
                Some(payload) = updates.recv() => {
                    self.emit(self.live_update(payload, turn.as_ref().map(|t| t.id.as_str())));
```

with:

```rust
            tokio::select! {
                // Biased: adapter output already received is emitted before
                // the prompt reply it preceded on the wire (every arm that
                // acts on the adapter drains what is queued first).
                biased;
                Some(payload) = updates.recv(), if burst < UPDATE_BURST => {
                    burst += 1;
                    self.emit(self.live_update(payload, turn.as_ref().map(|t| t.id.as_str())));
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                            continue;
                        }
                        seen_turns.insert(turn_id.clone());
```

with:

```rust
                            continue;
                        }
                        // Updates from before this turn are not part of it.
                        self.drain_updates(&mut updates, None);
                        seen_turns.insert(turn_id.clone());
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    return self.emit(SessionBody::SessionParked { reason: ParkReason::Idle });
                }
            }
```

with:

```rust
                    return self.emit(SessionBody::SessionParked { reason: ParkReason::Idle });
                }
                // A burst is over and nothing else was ready: back to the updates.
                _ = std::future::ready(()), if burst >= UPDATE_BURST => burst = 0,
            }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// ordering gap.
    fn drain_updates(&self, updates: &mut mpsc::UnboundedReceiver<Value>, turn: Option<&str>) {
        while let Ok(payload) = updates.try_recv() {
            self.emit(self.live_update(payload, turn));
        }
```

with:

```rust
    /// ordering gap.
    fn drain_updates(&self, updates: &mut mpsc::UnboundedReceiver<Value>, turn: Option<&str>) {
        // Only what is queued now: an adapter that keeps streaming cannot
        // hold the actor here.
        for _ in 0..updates.len() {
            match updates.try_recv() {
                Ok(payload) => self.emit(self.live_update(payload, turn)),
                Err(_) => break,
            }
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p hennery-testkit --test fake_acp --test host_session`
Expected: `fake_acp` 16 pass, `host_session` 47 pass. The two multi-thread ordering tests (`updates_never_land_outside_their_turn…`, `replayed_history_never_leaks…`) still pass: every arm that acts on the adapter still drains what was queued before it.

- [ ] **Step 6: Lint, test and commit**

```bash
cargo fmt --all && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked
cargo run -p hennery-proto --bin gen -- --check
git add crates/hennery-host crates/hennery-testkit
git commit -m "fix(host): cap updates in a row so a flooding adapter cannot hold off a cancel"
```

Expected: 241 tests pass.

---

## After this plan

**Obligations B2b hands on:**
- **Legacy model and mode switching.** Some adapters have no config options, only the crate's legacy `modes` (`session/set_mode`), or Codex's `session/set_model` (as `UntypedMessage`, §2.4). They belong to the adapter profiles (§6), together with reading `models` / `modes` into the extracts. Until then, such an adapter's requested model or mode ends as `host_note{config_failed}` ("offers no model option").
- **New-session pickers before a session exists.** `hello.agents[].catalog` and `host_agent_catalog` (§3.3, §8) come with the probes and profiles, refined from each `session_started`'s catalogue.
- **The rest of the catalogue.** `GET …/catalog` gains commands, plan and usage, with their extracts, and `session_catalog` gains its `commands` / `usage` columns (§7, §8).
- **List and detail items.** The session list item and `SessionDetail` gain `model` / `mode` once the frontend plan fixes the list item (§8's 1 KiB bound).
- **Live gates with real adapters.** "Model switch read-back" and "resume re-applies mode" (§12) still need a logged-in CI account and the pinned adapters. Task 8's tests are their fake-adapter stand-ins.
- **An adapter that answers switches without a catalogue** keeps its announced values stale until it sends a `config_option_update` (decision 3). If a real adapter does this routinely, reconsider applying the requested value to the host's copy. `POST …/config` with an empty read-back answers the stored catalogue, which then does not show the switch.
- **A timed-out live switch may still land.** After one, the next successful read-back is announced as current, although the late switch may still change the agent's config afterwards. Cover this in the live gate against the pinned adapters.
- **Spec amendments** listed under the decisions.
- **Carried from B2a:**
  - cancel and pending requests (`turn_cancelled`);
  - the `images` / `projects` capabilities;
  - `CANCEL_GRACE` is not configurable;
  - the `hello.capabilities` field doc still says "a closed list".
- **A flaky CLI test.** `sigint_to_ups_process_group_still_shuts_down_cleanly` (`crates/hennery/tests/cli.rs`) waits a fixed 300 ms for `up`'s children. It failed once, under a loaded parallel test run, while this plan was built, and passed on reruns. The fix is to poll for the children with a deadline.

Then, in order (unchanged from plan B2a):
- **(2) Permission and elicitation.** The pending set and the answer queue, including the teardown hooks plan A left out and `turn_cancelled`.
- **(3) Real auth and pairing.**
- **(4) Frontend shell.** Settle the generated TS optionals. Offer close for a `starting` session whose host is offline (B1). Hide park for hosts without the capability (§3.3). Show the config pickers from `GET …/catalog` and `catalog_changed`.
- **(5) Hats.**
- **(6) Gateway.**
- **(7) Distribution.**

---

_Generated with Claude AI — please review before distribution._
