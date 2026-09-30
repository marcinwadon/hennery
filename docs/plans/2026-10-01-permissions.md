# Permission and elicitation (plan 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When an agent asks the operator something (a permission for a tool call, or a form to fill in), the question reaches the collector as a pending request and waits there with no timeout. The operator's answer is queued durably and reaches the agent even if the host was away when it was given, and the card shows "answered" only once the host confirms the agent got it. Stop, park, close, a crashed adapter and a restarted host each close the open questions with a stated reason, so no card stays answerable after nobody is waiting for it any more.

**Architecture:**
- **Wire.** Three new session bodies: `pending_opened` (the ACP request verbatim, with a `pending` extract `{id, kind, option_ids?}`), `pending_resolved{delivered | cancelled, reason?}` and `answer_result{pending_id, request_id, delivered}`. Two new collector frames: `answer_permission` and `answer_elicitation`. On REST: `AnswerRequest`, `AnswerResponse`, `PendingItem`, and `SessionDetail.pending`.
- **Host.**
  - `initialize` advertises form elicitation as `{"form": {}}`.
  - The ACP client registers one untyped request handler. `session/request_permission` and `elicitation/create` join the actor's ordered inbound channel as questions; every other request is answered `-32601` at once.
  - The actor mints a `pending_id` for each question, emits `pending_opened` and keeps the responder. An answer is delivered at most once (`answer_result`, then `pending_resolved{delivered}`).
  - Cancel, park, close, idle reap and adapter exit answer the open questions `cancelled` and say why. The reaper never runs with a question open.
- **Collector.**
  - Migration 6 adds the canonical `pending` set and the durable `answer_queue`. Ingest keeps both from facts: `blocked` while a running turn has open questions, and a monotonic `delivered` fold.
  - A host restart, an unattached close, or a detach that left a question open cancel it collector-side (`pending_cancelled`).
  - `POST …/pending/{pending_id}/answer` queues the answer and sends it at once to a ready host. Every reconciliation drains whatever is still owed.
  - SSE `pending_changed` follows every step.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, agent-client-protocol 2.2.0 / schema 1.9.1 (`Responder`, `RequestCancellation`, `UntypedMessage`, `ElicitationCapabilities`), rusqlite 0.40, uuid 1 (v7), schemars/ts-rs codegen. The only dependency change is the workspace's own `uuid`, added to `hennery-host`. Nix flake dev shell.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md). The relevant sections are:
- §2.3 (the exit watcher resolves every pending request `adapter_lost`, after the turn's end and before `adapter_exited`);
- §2.4 (inbound requests are dispatched without blocking the reader; untyped handlers forward `params` verbatim, and a typed copy only fills the extracts);
- §2.5 (the client-side methods: `session/request_permission`, `elicitation/create`, "anything else: `-32601`");
- §3.2 (`pending_opened`, `pending_resolved`, `answer_result`; the `pending` extract);
- §3.3 (`answer_permission`, `answer_elicitation`, completed by `answer_result`);
- §3.4 ("Answers have no waiter: they are queued durably");
- §4.6 (the whole of it: no timeout, `pending_id`, `{"form": {}}` (P-19), validation, the answer queue, the pending set, the monotonic fold);
- §4.2 and §4.4 (`active/running` ⇄ `active/blocked`; a pending request is inside its turn);
- §4.7, §4.8 (the reaper never reaps a turn in flight; park and close cancel pending requests);
- §5.1 step 4 (drain the answer queue after reconciliation), §5.2 (a host restart cancels them `host_restarted`), §5.3 (a presumed park keeps them `open`);
- §8 (`pending`, `answer_queue`, `answer_submitted`);
- §9 (`POST …/pending/{pending_id}/answer`, `SessionDetail` with pending, SSE `pending_changed`);
- §10 (the `activity → blocked` edge, recorded only: push delivery is out);
- §12 (scenarios 5, 7, 8, 9 and 10; the live gate "form elicitation round trip with `answer_result{delivered: true}`", as far as the fake adapter can stand in for a real one).
- It also relies on the umbrella spec [`2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md): §6.5 (no timeout; the cancellation is explicit on the card) and §6.8 (delivery acknowledgement, the monotonic fold).

It builds on the executed [session config plan](2026-09-30-session-config.md) (plan B2b). Read its "Execution status" and "After this plan" first. Its code wins over its task text, and every anchor below was taken from that code (`main` at `4659c27`, which merged it).

**Status:** not executed. Every code block below was built and tested in a scratch copy of `4659c27`. The plan was then replayed from its own text, task by task, onto a fresh copy of `4659c27`. After every task the replay ran fmt, clippy (also on the shipped binary with test hooks off), the workspace tests and the codegen check. It ends with 304 tests, up from 258. The review amendments of 2026-10-01 were replayed the same way. Every new timing-sensitive test passed with four copies of its test binary running at once.

## Scope

This is **plan (2)**, permission and elicitation, as plans A, B1, B2a and B2b scoped it in their "After this plan". It fits in seven right-sized tasks, so it is not split.

**In:**
- Wire: the three bodies, the two frames, the `pending` extract, the REST answer types, `PendingItem` and `SessionDetail.pending`.
- Fake adapter: scripted `session/request_permission` and `elicitation/create` asks, answered one at a time or all open at once. Each answer is echoed as a chunk. Elicitation is asked only of a client that advertised `elicitation.form` (P-19). It can ask during a load (and make the load wait for the answer), crash with a question open, withdraw its question, offer an option kind the schema does not know, and send a method no client serves.
- Host:
  - the elicitation capability;
  - the untyped request handler, with `-32601` for everything else;
  - questions in wire order, held back until `session_started`;
  - answers, at most one delivered per question;
  - a question the adapter withdraws (`$/cancel_request`);
  - cancellation on stop, park, close, idle reap and adapter exit;
  - no reap with a question open.
- Collector:
  - migration 6;
  - the pending set and `blocked`, and the answer queue with its fold;
  - collector-side cancellation (`host_restarted`, unattached close, a detach backstop);
  - the answer endpoint, the drain after reconciliation, a host's refusal of an answer, `SessionDetail.pending` and SSE `pending_changed`.

**Out** (later plans; see "After this plan"):
- push delivery for the `blocked` edge;
- the frontend's cards;
- `fs/*` and `terminal/*` (they are answered `-32601` for now);
- `DELETE /api/sessions/{id}` and with it the deletion of pending rows (§4.10);
- real auth, hats, gateway, distribution.

**Where the earlier hand-offs land:**

| Hand-off (plan) | Here |
|---|---|
| The pending set and the answer queue (A, B1, B2a, B2b "Then, in order") | Tasks 1, 5, 6 |
| Cancel pending requests `adapter_lost` / `session_parked` / `session_closed` (A) | Task 4 (host), Task 5 (collector backstop) |
| `host_restarted`, including a restart found through a presumed session (A, B1) | Task 5 |
| Presumed park keeps pending requests `open` (B1) | Task 5 |
| Drain the answer queue after reconciliation (A) | Task 6 |
| `cancel_turn` resolves the turn's pending requests `turn_cancelled` (B2a) | Task 4 |
| `SessionDetail` gains pending requests (B1) | Tasks 1, 6 |
| The fake adapter needs `session/request_permission` and elicitation requests | Task 2 |
| Scenarios 5, 7, 8, 9, 10 and the elicitation live gate (§12) | Tasks 4, 6, 7 |
| B2b's other obligations (legacy model switching, pickers, the unbounded drain, the flaky CLI test, …) | Unchanged, see "After this plan" |

## Decisions this plan makes where the spec is silent

**Amendments (2026-10-01 review):**
- Decision 4: a permission's `option_ids` are read from the raw request, not from a typed copy. The 400 text for a request without them says "stop, park or close the session".
- Decision 7 and decision 10: a host's refusal of an answer is logged and is no verdict.
- Decision 2: a start that runs out of time names the questions it held back.
- Decision 15 is new: the adapter withdrawing its own question (`$/cancel_request`).
- "After this plan" gains three hand-offs: push for questions asked outside a turn, host revoke, and `elicitation/complete`.

Reviewed and confirmed (with the amendments above) on 2026-10-01 by a stronger-model review on the maintainer's behalf. It checked the decisions against agent-client-protocol 2.2.0 and schema 1.9.1. Four facts about the crate shaped them:
- `Responder` sends nothing when an individual request's responder is dropped. The adapter then waits for good.
- An `UntypedMessage` request handler sees every method.
- The crate's default handler for the agent side answers `Handled::No { retry: true }` for any message that names a session, so the message is held for a per-session handler that hennery never registers. Today an adapter's `fs/read_text_file` or `terminal/create` therefore hangs instead of getting §2.5's `-32601` (decision 3).
- `PermissionOptionKind` is `#[non_exhaustive]` with no catch-all, and `RequestPermissionRequest.options` is a plain `Vec`: one option of a new kind fails the whole typed parse (decision 4). A peer's `$/cancel_request` reaches the request's `RequestCancellation`, which `Responder::cancellation()` exposes (decision 15).

The tasks implement the decisions as written here.

1. **The kind of a question rides in its `pending` extract.** §3.2 lists `kind` as a field of `pending_opened`, but `kind` is already the tag of every session body. So `pending_opened` is `{pending_id, indexed, payload}`, and `indexed.pending = {id, kind, option_ids?}` carries the kind, as §4.6 describes the extract. The host always fills the extract: it knows the id and the kind without parsing anything.
2. **Questions that arrive before `session_started` are held, then opened right after it.** That covers `initialize`, `session/new`, `session/load` and the start's switches.
   - They are opened in wire order, together with the start's early updates. The collector has no attached session to hang them on before then.
   - An adapter that blocks its load on such a question costs the start its 75 s deadline and ends `start_failed`. **Amended:** that `start_failed` names what was held back: "…; the agent asked N question(s) during start-up (permission/elicitation) that hennery cannot show before the session exists". See "After this plan".
3. **hennery answers every other adapter request itself, with `-32601 Method not found`.**
   - This is §2.5's "anything else". Falling through to the crate is not enough: it would hold any request carrying a `sessionId` forever (see above).
   - This also fixes today's hang for `fs/*` and `terminal/*` until those methods are implemented.
4. **Ids and extracts.**
   - `pending_id` is a UUIDv7 from the workspace's `uuid` crate. It is globally unique, random beyond its time prefix, and orders by creation. §4.6 says "random UUID"; v7 satisfies it, and it needs no new dependency or feature.
   - **Amended.** A permission's `option_ids` are read from the raw params: every string `options[i].optionId`, skipping an entry without one. A typed copy (§2.4) would fail on a single option of a kind this build does not know, and then every answer would be refused. There are no `option_ids` only when `options` is missing or not an array. The question still opens then, but the collector accepts no answer to it (400 `invalid`, "stop, park or close the session"): it can validate only against stored option ids (§3.2).
5. **A stop answers every open question `cancelled`.**
   - On the first `session/cancel` for a turn, every open question is answered cancelled: a permission gets the `cancelled` outcome, an elicitation the `cancel` action. Each is announced as `pending_resolved{cancelled, turn_cancelled}`. ACP asks this of a client after `session/cancel`.
   - A question the same turn asks after the cancel is cancelled the moment it opens: the operator asked to stop.
   - A repeated cancel changes nothing.
6. **Teardown order and the reaper.**
   - On park, close and adapter exit, the turn's `turn_ended` comes first, then the `pending_resolved` of every open question, then (on an exit) `adapter_exited`, then `session_parked` / `session_closed`. That is §2.3's order, and §4.8's.
   - The idle reaper never parks a session with a question open, in a turn or not: a question has no timeout.
   - Host shutdown still emits nothing. The collector cancels those questions `host_restarted` after the next handshake.
7. **Answers on the host.**
   - An answer for a question the actor holds is delivered: `answer_result{delivered: true}`, then `pending_resolved{delivered}`.
   - Any other answer changes nothing and is `answer_result{delivered: false}`. That covers one already answered, one cancelled, one asked of an earlier adapter, or a kind mismatch the collector's validation makes unreachable.
   - An answer for a session with no live actor is refused with a correlated `error{not_attached}`, like a prompt. **Amended:** the collector logs that refusal; it is not a verdict (decision 10).
   - The connection task does not emit an outboxed `answer_result` for such a session. `hello_ack` fast-forwards only attached sessions, so after an outbox loss that frame's seq could collide with a committed one.
8. **`blocked`, and the push edge.**
   - `activity` becomes `blocked` when a `pending_opened` applies to a `running` session, and returns to `running` when its last open question resolves. `turn_ended` makes it `idle` as before.
   - A question asked outside any turn leaves `activity` alone.
   - That transition on ingest is §10's `activity → blocked` edge. This plan only records it; the push plan hooks there.
9. **Collector-side cancellation is a new collector event, `pending_cancelled{pending_id, reason}`, one per question.** It extends §8's closed list, next to `turn_ended_synthesized`. It is written:
   - for a host restart, when reconciliation finds an attached session missing (`host_restarted`), including one it had presumed parked;
   - for a close of an unattached session (`session_closed`);
   - as a backstop, when a host's `session_parked` / `session_closed` applies while a question is still open. `adapter_exited` gives `adapter_lost`, idle or operator give `session_parked`, a close gives `session_closed`.

   A presumed park leaves questions `open` (§5.3).
10. **The answer queue's verdict.**
    - `answer_queue.delivered` is NULL until a verdict, which folds §8's `state` column into it. **Amended:** a verdict comes only from:
      - `answer_result`, folded so `true` sticks;
      - the question's resolution: cancellation gives `false` (nobody will take it).

      A host's refusal of the answer's request is logged and leaves `delivered` NULL, so the answer goes again after the next handshake while its question is open.
    - After every reconciliation, and only after `mark_ready`, the queue sends every answer still NULL whose question is still open, oldest first. So an answer lost with a connection goes again after the next handshake. The host dedupes by `pending_id`.
    - The endpoint also sends the answer at once to a host that is ready. An answer racing a reconciliation is therefore sent by one path or by both, never by neither.
11. **The answer endpoint.**
    - `POST /api/sessions/{id}/pending/{pending_id}/answer` accepts an answer to any `open` question of that session, whatever the lifecycle and whether the host is connected (scenario 9). It answers:
      - 202 `{pending_id, request_id}`;
      - 404 `not_found`;
      - 409 `not_open` / `already_answered`;
      - 400 `invalid` for an option the question does not offer, the wrong kind of answer, or content on anything but an `accept` (content must be an object);
      - 422 for a body that is neither kind.
    - The check and the insert are one transaction, and the queue's key is the pending id, so two concurrent answers queue exactly one.
    - Elicitation content is not checked against the requested schema: that is the adapter's call.
12. **The SSE `pending_changed` message and the detail.**
    - `pending_changed` follows every event about a question: `pending_opened`, `pending_resolved`, `pending_cancelled`, `answer_submitted` and `answer_result`. It carries its event's id, and its data is the question as it stands now (`PendingItem`).
    - A replay from `Last-Event-ID` therefore sends it too, with the latest state rather than the historic one. The client ends in the right state either way.
    - `SessionDetail.pending` lists the open questions, oldest first.
13. **Every elicitation is forwarded, and only form elicitation is advertised.** An adapter that sends URL-mode or request-scoped elicitations anyway still reaches the operator (the payload is opaque) and can still be accepted, declined or cancelled.
14. **Facts that come too late are stored, not applied.**
    - A `pending_opened` for a detached session, or for a turn that has ended, is not applied.
    - A `pending_resolved` or `answer_result` for a question that is not open, or has no queued answer, is not applied either.

15. **The adapter may withdraw its own question** (`$/cancel_request`, new with the review).
    - `open_question` takes the responder's `RequestCancellation` and spawns a watcher. When the peer cancels the request, the watcher sends `QuestionWithdrawn{pending_id}` into the actor's ordered channel. The watcher is aborted when the question is answered or cancelled.
    - The actor removes the question, answers the request with the standard `-32800` cancellation error, and emits `pending_resolved{cancelled, reason: agent_withdrew}`. `agent_withdrew` is a new reason.
    - Without this the card would stay answerable, an answer would report `delivered: true` to nobody, and the open question would keep the reaper away for good.

**Spec drift to reconcile after review:** these are refinements of ACP core §3.2, §3.3, §4.6, §8 and §9, and the spec text should be amended to match:
- `pending_opened` without a body `kind`;
- the `pending_cancelled` collector event;
- `answer_queue.delivered` in place of `state`;
- the 202 body and the codes `not_open` / `already_answered` / `invalid`;
- `error{not_attached}` for an answer to a session with no live actor;
- hennery's own `-32601` handler;
- `pending_changed` carrying the current `PendingItem`;
- `pending_id` as UUIDv7;
- the reason `agent_withdrew` (decision 15);
- `option_ids` read from the raw request (decision 4).

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- `cargo fmt --all --check` (`max_width = 120`), `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo clippy -p hennery --locked -- -D warnings` (test hooks off) pass after every task. So does `cargo test --workspace --locked`.
- Generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated with `cargo run -p hennery-proto --bin gen` whenever a wire type changes, and must pass `cargo run -p hennery-proto --bin gen -- --check`. In `codegen.rs`, new root types (frames, REST payloads) go in both `add!` lists, and new nested types go in the `render_ts` list.
- ACP payloads are forwarded verbatim as `serde_json::Value` and never scrubbed (ACP core §2.3, §3.2). The collector never parses them: everything it keeps about a question comes from the `pending` extract.
- "Every state-bearing fact from the host is a sequenced `session` frame through the outbox; only rejections (`error`) bypass it" (ACP core §3.1, §3.3). `pending_opened`, `pending_resolved` and `answer_result` are such facts.
- "The host answers the adapter only when the operator answers, the turn is cancelled, the session closes or parks, or the adapter is lost. **No timeout.**" (ACP core §4.6)
- "The elicitation client capability is advertised as `{"form": {}}`, never a boolean." (ACP core §4.6, P-19)
- "The collector validates `action ∈ {accept, decline, cancel}` and the option id against the stored `option_ids`" (ACP core §4.6). "An answer for a pending that is not `open`, or that already has a queued answer, → 409 (`not_open` / `already_answered`)."
- "Verdicts from several clients are folded monotonically: `delivered` sticks and a later `delivered: false` never overwrites it" (ACP core §4.6, umbrella §6.8).
- The collector sends no request to a host before that connection's post-`resend_complete` reconciliation (ACP core §5.1 step 4). The answer queue drains after it.
- The collector acks a frame only after its transaction commits; ingest is idempotent on `(session_id, seq)`.
- No global installs: tooling comes from the flake dev shell.
- Commits follow Conventional Commits (`feat(host): …`). They use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the five inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **Stop pressed while the agent waits for an answer, or an agent that asks again after the stop.** Expected: the question closes at once as `cancelled` (`turn_cancelled`), and the adapter hears `cancelled`, so the turn ends `cancelled` without waiting out the cancel grace. A question asked after the stop is cancelled as it opens. A late answer is refused 409 `not_open`, and the host reports `delivered: false`. (Task 4: `a_cancel_answers_the_open_questions_cancelled_and_the_turn_ends_cancelled`, `a_question_asked_after_the_cancel_is_cancelled_at_once`; Task 7: `stop_with_a_question_open_ends_the_turn_cancelled_and_closes_the_question`)
2. **The same question answered from two tabs, or an answer resent after a reconnect.** Expected: the second submit is 409 `already_answered`. An answer that arrives twice reaches the agent once, and `delivered: true` is never overwritten by a later `false`. (Task 3: `an_answer_is_delivered_once_and_one_for_a_question_not_open_is_not`; Task 5: `a_delivered_verdict_sticks_and_a_later_false_does_not_overwrite_it`; Task 6: `an_answer_is_queued_sent_to_the_host_and_its_verdict_recorded`, `an_answer_given_while_the_host_is_offline_is_sent_after_its_next_handshake`)
3. **An answer given while the host is offline, presumed parked, or still reconciling.** Expected: 202, queued. It is sent only after the host's `resend_complete`, and resent after each handshake until a verdict comes. The adapter still waiting for it gets it (scenarios 8, 9). (Task 6: `an_answer_given_while_the_host_is_offline_is_sent_after_its_next_handshake`; Task 7: `a_question_outlasts_its_host_being_away_and_an_answer_given_meanwhile_is_delivered`)
4. **The adapter crashes, the host restarts, or the agent withdraws its question, while a question is open, maybe with its answer already queued.** Expected: the question is cancelled `adapter_lost` or `host_restarted`, in §2.3's order. A queued answer for it is never sent and gets `delivered: false`, and a new answer is 409 `not_open`. Nothing is re-spawned. A withdrawn question closes `agent_withdrew`, and an answer to it reaches nobody. (Task 4: `an_adapter_lost_with_a_question_open_cancels_it_adapter_lost`, `a_question_the_agent_withdraws_closes_and_an_answer_reaches_nobody`; Task 5: `a_host_restart_cancels_open_questions_and_a_presumed_park_keeps_them`, `a_detach_or_an_unattached_close_cancels_whatever_is_still_open`; Task 6: `a_host_restart_cancels_the_open_questions_and_drops_their_queued_answers`; Task 7: `an_adapter_crash_with_a_question_open_cancels_it_adapter_lost`, `a_host_restart_with_a_question_open_cancels_it_host_restarted`)
5. **An adapter request hennery does not serve (`fs/*`, `terminal/*`, a method newer than this build), a permission option of a kind this build does not know, or a question asked while the session loads.** Expected: the unserved request is answered `-32601` at once and the turn goes on; nothing hangs. The unknown option kind keeps every option id answerable. A question asked during the load opens right after `session_started`, outside any turn, and can be answered. It keeps the reaper away however long it waits (scenario 10). (Task 3: `a_request_the_host_does_not_serve_is_refused_method_not_found`, `a_permission_with_an_option_kind_this_build_does_not_know_keeps_its_option_ids`, `option_ids_are_read_from_the_raw_request`, `a_question_asked_while_the_session_loads_opens_after_session_started`, `a_start_that_runs_out_of_time_says_which_questions_it_held_back`; Task 4: `the_reaper_never_parks_a_session_with_a_question_open`)

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/frames.rs` | `PendingKind`, `PendingResolution`, `PendingReason`, `ElicitationAction`, `PendingExtract`, `Indexed.pending`; `PendingOpened`, `PendingResolved`, `AnswerResult`; `AnswerPermission`, `AnswerElicitation` | 1 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs` | `PendingState`, `PendingItem`, `AnswerRequest`, `AnswerResponse`, `SessionDetail.pending` | 1 |
| `crates/hennery-testkit/src/lib.rs`, `src/bin/hennery-fake-acp.rs` | `FakeAsk` (with `FuturePermission`), `asks`, `asks_at_once`, `crash_while_asking`, `ask_on_load`, `ask_on_load_waits`, `withdraw_asks` | 2 |
| `crates/hennery-host/Cargo.toml`, `Cargo.lock` | `uuid` for `hennery-host` | 3 |
| `crates/hennery-host/src/session.rs` | `Answer`, `SessionCmd::Answer`, `Inbound::Question`, `Early`, `Questions`, `client_capabilities`, `option_ids`, `held_questions`, `open_question`, `answer` (3); `cancel_questions`, `resolve_cancelled`, the teardown and reaper hooks, `Inbound::QuestionWithdrawn`, `Watcher`, `withdraw_question` (4) | 3, 4 |
| `crates/hennery-host/src/connection.rs` | `answer_*` dispatch | 1, 3 |
| `crates/hennery-sessions/src/store.rs` | Migration 6, `AnswerSubmission`, `QueuedAnswer`, `resolve_pending`, `cancel_open_pending`, the three ingest arms, `open_pending`, `pending_item`, `submit_answer`, `answers_to_send` | 1, 5 |
| `crates/hennery-sessions/src/hub.rs`, `ws.rs`, `api.rs` | `Hub::notify`; the drain after reconciliation, and logging refused answers; `POST …/answer`, `SessionDetail.pending`, `pending_changed` | 1, 6 |
| `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/{fake_acp,host_session,host_connection,reconcile,e2e}.rs`, `crates/hennery-sessions/tests/store.rs` | Tests | all |

All commands run from the repository root inside the dev shell (`nix develop`, or direnv). Work on a feature branch off `main` (e.g. `feat/permissions`), once `feat/session-config` has merged. Each task leaves the workspace compiling, clippy-clean and green.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "Replace the whole of `path` with:" overwrites the file.
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point (earlier blocks of the same task already applied, in order), then "with:" and its replacement.
- "Run this rewrite:" is followed by a shell block that edits files mechanically; run it from the repository root.

Other "Run:" lines only check; they change nothing. The plan was replayed exactly this way, from its own text, onto `4659c27`.

---

### Task 1: Wire types for questions, answers and verdicts

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`
- Modify (so every exhaustive match compiles): `crates/hennery-sessions/src/store.rs` (`ingest`, `body_kind`), `crates/hennery-sessions/src/api.rs` (`session_detail`), `crates/hennery-host/src/connection.rs` (`handle`), `crates/hennery-testkit/tests/host_session.rs` (`kinds`)
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: `SessionBody`, `CollectorFrame`, `Indexed`, `SessionDetail` as `4659c27` has them.
- Produces (`hennery_proto::frames`):
  - `enum PendingKind { Permission, Elicitation }`
  - `enum PendingResolution { Delivered, Cancelled }`
  - `enum PendingReason { TurnCancelled, SessionClosed, SessionParked, AdapterLost, HostRestarted, AgentWithdrew }`
  - `enum ElicitationAction { Accept, Decline, Cancel }`
  - `struct PendingExtract { id: String, kind: PendingKind, option_ids: Option<Vec<String>> }`

  All of them are snake_case on the wire, and all are `Copy` except `PendingExtract`.
- Produces: `Indexed.pending: Option<PendingExtract>`.
- Produces: new `SessionBody` variants:
  - `PendingOpened { pending_id: String, indexed: Indexed, payload: Value }`
  - `PendingResolved { pending_id: String, resolution: PendingResolution, reason: Option<PendingReason> }`
  - `AnswerResult { pending_id: String, request_id: String, delivered: bool }`
- Produces: new `CollectorFrame` variants:
  - `AnswerPermission { request_id, session_id, pending_id, option_id: String }`
  - `AnswerElicitation { request_id, session_id, pending_id: String, action: ElicitationAction, content: Option<Value> }`
- Produces (`hennery_proto::rest`):
  - `enum PendingState { Open, Delivered, Cancelled }`
  - `struct PendingItem { pending_id, session_id: String, kind: PendingKind, state: PendingState, reason: Option<PendingReason>, turn_id: Option<String>, option_ids: Option<Vec<String>>, payload: Value, answered: bool, delivered: Option<bool> }`
  - `#[serde(untagged)] enum AnswerRequest { Permission { option_id: String }, Elicitation { action: ElicitationAction, content: Option<Value> } }`
  - `struct AnswerResponse { pending_id, request_id: String }`
  - `SessionDetail.pending: Vec<PendingItem>`, always serialized
- Interim, until the tasks that own them:
  - the store keeps the three new facts as events with no transition (Task 5 replaces that);
  - `session_detail` answers `pending: []` (Task 6 fills it);
  - the host answers `answer_*` with `error{unsupported}` (Task 3 replaces that).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-proto/tests/frames.rs`:

```rust
// Plan (2): permission and elicitation (ACP core §3.2, §3.3, §4.6).

#[test]
fn pending_bodies_use_the_spec_field_names() {
    use hennery_proto::frames::{PendingExtract, PendingKind, PendingReason, PendingResolution};
    let opened = SessionBody::PendingOpened {
        pending_id: "p1".into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(PendingExtract {
                id: "p1".into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
            }),
            ..Indexed::default()
        },
        payload: json!({"sessionId": "a1", "options": [], "_meta": {"x": 1}}),
    };
    // `kind` is the body's tag, so the request's own kind rides in the
    // `pending` extract.
    let expected = json!({
        "kind": "pending_opened", "pending_id": "p1",
        "indexed": {"turn_id": "t1", "pending": {"id": "p1", "kind": "permission", "option_ids": ["allow", "reject"]}},
        "payload": {"sessionId": "a1", "options": [], "_meta": {"x": 1}}
    });
    assert_eq!(serde_json::to_value(&opened).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), opened);
    let cancelled = SessionBody::PendingResolved {
        pending_id: "p1".into(),
        resolution: PendingResolution::Cancelled,
        reason: Some(PendingReason::TurnCancelled),
    };
    let expected =
        json!({"kind": "pending_resolved", "pending_id": "p1", "resolution": "cancelled", "reason": "turn_cancelled"});
    assert_eq!(serde_json::to_value(&cancelled).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), cancelled);
    let result = SessionBody::AnswerResult {
        pending_id: "p1".into(),
        request_id: "r9".into(),
        delivered: true,
    };
    let expected = json!({"kind": "answer_result", "pending_id": "p1", "request_id": "r9", "delivered": true});
    assert_eq!(serde_json::to_value(&result).unwrap(), expected);
    assert_eq!(serde_json::from_value::<SessionBody>(expected).unwrap(), result);
}

#[test]
fn answer_frames_use_the_spec_field_names() {
    use hennery_proto::frames::ElicitationAction;
    let permission = CollectorFrame::AnswerPermission {
        request_id: "r".into(),
        session_id: "s".into(),
        pending_id: "p".into(),
        option_id: "allow".into(),
    };
    let expected = json!({"type": "answer_permission", "request_id": "r", "session_id": "s", "pending_id": "p", "option_id": "allow"});
    assert_eq!(serde_json::to_value(&permission).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), permission);
    let elicitation = CollectorFrame::AnswerElicitation {
        request_id: "r".into(),
        session_id: "s".into(),
        pending_id: "p".into(),
        action: ElicitationAction::Accept,
        content: Some(json!({"name": "hennery"})),
    };
    let expected = json!({
        "type": "answer_elicitation", "request_id": "r", "session_id": "s", "pending_id": "p",
        "action": "accept", "content": {"name": "hennery"}
    });
    assert_eq!(serde_json::to_value(&elicitation).unwrap(), expected);
    assert_eq!(serde_json::from_value::<CollectorFrame>(expected).unwrap(), elicitation);
    let declined: CollectorFrame = serde_json::from_value(json!({
        "type": "answer_elicitation", "request_id": "r", "session_id": "s", "pending_id": "p", "action": "decline"
    }))
    .unwrap();
    assert!(matches!(
        declined,
        CollectorFrame::AnswerElicitation { content: None, .. }
    ));
}

#[test]
fn a_rest_answer_is_an_option_or_an_elicitation_action() {
    use hennery_proto::frames::ElicitationAction;
    use hennery_proto::rest::AnswerRequest;
    let option: AnswerRequest = serde_json::from_value(json!({"option_id": "allow"})).unwrap();
    assert_eq!(
        option,
        AnswerRequest::Permission {
            option_id: "allow".into()
        }
    );
    let accept: AnswerRequest = serde_json::from_value(json!({"action": "accept", "content": {"n": 1}})).unwrap();
    assert_eq!(
        accept,
        AnswerRequest::Elicitation {
            action: ElicitationAction::Accept,
            content: Some(json!({"n": 1}))
        }
    );
    let decline: AnswerRequest = serde_json::from_value(json!({"action": "decline"})).unwrap();
    assert!(matches!(decline, AnswerRequest::Elicitation { content: None, .. }));
    for bad in [json!({}), json!({"action": "maybe"}), json!({"option_id": 3})] {
        assert!(serde_json::from_value::<AnswerRequest>(bad.clone()).is_err(), "{bad}");
    }
}
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        CollectorFrame::SetConfig {
            request_id: "r".into(),
            session_id: "s".into(),
            config_id: "model".into(),
            value: ConfigValue::Id("large".into()),
        },
    ];
```

with:

```rust
        CollectorFrame::SetConfig {
            request_id: "r".into(),
            session_id: "s".into(),
            config_id: "model".into(),
            value: ConfigValue::Id("large".into()),
        },
        CollectorFrame::AnswerPermission {
            request_id: "r".into(),
            session_id: "s".into(),
            pending_id: "p".into(),
            option_id: "allow".into(),
        },
        CollectorFrame::AnswerElicitation {
            request_id: "r".into(),
            session_id: "s".into(),
            pending_id: "p".into(),
            action: hennery_proto::frames::ElicitationAction::Cancel,
            content: None,
        },
    ];
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": { "turn_id": turn, "state": "started" }
        })
```

with:

```rust
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": { "turn_id": turn, "state": "started" }, "pending": []
        })
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        open_turn: Some(OpenTurn {
            turn_id: "t".into(),
            state: "started".into(),
        }),
    };
    assert_eq!(
        serde_json::to_value(&detail).unwrap(),
        json!({
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp",
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": {"turn_id": "t", "state": "started"}
        })
    );
```

with:

```rust
        open_turn: Some(OpenTurn {
            turn_id: "t".into(),
            state: "started".into(),
        }),
        pending: vec![],
    };
    assert_eq!(
        serde_json::to_value(&detail).unwrap(),
        json!({
            "session_id": "s", "host_id": "h", "agent": "claude", "cwd": "/tmp",
            "lifecycle": "active", "activity": "running", "presumed_parked": false,
            "open_turn": {"turn_id": "t", "state": "started"}, "pending": []
        })
    );
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-proto --test frames --locked`
Expected: FAIL to compile, with `error[E0432]: unresolved imports hennery_proto::frames::PendingExtract, hennery_proto::frames::PendingKind, …`.

- [ ] **Step 3: Add the wire types, and keep every match compiling**

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
/// The value of one config option (ACP `session/set_config_option`): a
```

with:

```rust
/// What a pending request asks the operator (ACP core §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    /// `session/request_permission`: pick one of the offered options.
    Permission,
    /// `elicitation/create`: fill in a form, or decline.
    Elicitation,
}

/// How a pending request ended (ACP core §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingResolution {
    /// The operator's answer reached the waiting adapter.
    Delivered,
    /// The adapter was told the question is off (see `PendingReason`).
    Cancelled,
}

/// Why a pending request was cancelled (ACP core §4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    TurnCancelled,
    SessionClosed,
    SessionParked,
    AdapterLost,
    HostRestarted,
    /// The adapter withdrew its own question (`$/cancel_request`).
    AgentWithdrew,
}

/// The operator's answer to an elicitation (ACP `elicitation/create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

/// The `pending` extract (ACP core §3.2): what the collector needs to hold
/// a pending request and validate its answer, without reading the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingExtract {
    pub id: String,
    pub kind: PendingKind,
    /// The permission's option ids. Absent for an elicitation, and for a
    /// permission request whose options hennery could not parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_ids: Option<Vec<String>>,
}

/// The value of one config option (ACP `session/set_config_option`): a
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    /// The current value of every other option, by config id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_axes: Option<BTreeMap<String, ConfigValue>>,
}
```

with:

```rust
    /// The current value of every other option, by config id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_axes: Option<BTreeMap<String, ConfigValue>>,
    /// On `pending_opened`: the request's id, kind and option ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingExtract>,
}
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    ConfigApplied {
        request_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
}
```

with:

```rust
    ConfigApplied {
        request_id: String,
        #[serde(default)]
        indexed: Indexed,
    },
    /// The adapter asked the operator something (ACP core §4.6): its ACP
    /// request verbatim in `payload`, with `indexed.pending`. It waits, with
    /// no timeout, until it is answered or cancelled. Its kind is
    /// `indexed.pending.kind` (the body's own `kind` is its tag).
    PendingOpened {
        pending_id: String,
        #[serde(default)]
        indexed: Indexed,
        #[ts(type = "unknown")]
        payload: Value,
    },
    /// A pending request is over: answered (`delivered`), or cancelled for
    /// `reason`.
    PendingResolved {
        pending_id: String,
        resolution: PendingResolution,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<PendingReason>,
    },
    /// The host's verdict on one answer (umbrella §6.8): `delivered` if the
    /// adapter was still waiting for it. A delivered answer is followed by
    /// `pending_resolved{delivered}`.
    AnswerResult {
        pending_id: String,
        request_id: String,
        delivered: bool,
    },
}
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
    /// The operator's choice for a permission request. Completed by
    /// `answer_result` (ACP core §4.6); it has no collector waiter.
    AnswerPermission {
        request_id: String,
        session_id: String,
        pending_id: String,
        option_id: String,
    },
    /// The operator's answer to an elicitation; `content` only with
    /// `accept`. Completed by `answer_result`, like a permission's.
    AnswerElicitation {
        request_id: String,
        session_id: String,
        pending_id: String,
        action: ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown")]
        content: Option<Value>,
    },
    Ack {
        session_id: String,
        #[ts(type = "number")]
        ack_seq: u64,
    },
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
use crate::frames::{ConfigValue, Indexed, SessionConfig};
```

with:

```rust
use crate::frames::{ConfigValue, ElicitationAction, Indexed, PendingKind, PendingReason, SessionConfig};
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
/// `GET /api/sessions/{id}` (ACP core §9): the list item plus the open turn.
/// Pending requests join it with permission handling.
```

with:

```rust
/// `GET /api/sessions/{id}` (ACP core §9): the list item, the open turn and
/// the pending requests still open.
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    #[ts(type = "OpenTurn | undefined", optional)]
    pub open_turn: Option<OpenTurn>,
}
```

with:

```rust
    #[ts(type = "OpenTurn | undefined", optional)]
    pub open_turn: Option<OpenTurn>,
    /// Open pending requests, oldest first: what the operator can answer.
    #[serde(default)]
    pub pending: Vec<PendingItem>,
}

/// Where a pending request stands (ACP core §4.6): `open`, then
/// `delivered` or `cancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PendingState {
    Open,
    Delivered,
    Cancelled,
}

/// One pending request, as the collector holds it: an entry of
/// `SessionDetail.pending`, and the data of SSE `pending_changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PendingItem {
    pub pending_id: String,
    pub session_id: String,
    pub kind: PendingKind,
    pub state: PendingState,
    /// Why it was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "PendingReason | undefined", optional)]
    pub reason: Option<PendingReason>,
    /// The turn it was asked in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub turn_id: Option<String>,
    /// A permission's option ids: the only valid answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string[] | undefined", optional)]
    pub option_ids: Option<Vec<String>>,
    /// The adapter's ACP request, verbatim.
    #[ts(type = "unknown")]
    pub payload: Value,
    /// An answer has been accepted for it (at most one is).
    pub answered: bool,
    /// The host's verdict on that answer, once one arrived. `true` sticks
    /// (umbrella §6.8): a card shows "answered" only then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub delivered: Option<bool>,
}

/// `POST /api/sessions/{id}/pending/{pending_id}/answer` (ACP core §9): an
/// option for a permission request, or an action for an elicitation, with
/// the form's content when it accepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AnswerRequest {
    Permission {
        option_id: String,
    },
    Elicitation {
        action: ElicitationAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown", optional)]
        content: Option<Value>,
    },
}

/// 202 to an answer: it is queued for the session's host, and delivered now
/// or on the host's next connection (ACP core §4.6). The verdict follows on
/// the session stream as `pending_changed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AnswerResponse {
    pub pending_id: String,
    /// Carried by the host's `answer_result` for this answer.
    pub request_id: String,
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::ConfigRequest,
        rest::SessionCatalog,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::ConfigRequest,
        rest::SessionCatalog,
        rest::PendingItem,
        rest::AnswerRequest,
        rest::AnswerResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        frames::ParkReason,
        frames::Indexed,
```

with:

```rust
        frames::ParkReason,
        frames::PendingKind,
        frames::PendingResolution,
        frames::PendingReason,
        frames::ElicitationAction,
        frames::PendingExtract,
        frames::Indexed,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::ConfigRequest,
        rest::SessionCatalog,
    );
    out
}
```

with:

```rust
        rest::ConfigRequest,
        rest::SessionCatalog,
        rest::PendingState,
        rest::PendingItem,
        rest::AnswerRequest,
        rest::AnswerResponse,
    );
    out
}
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            // Diagnostics only, with no transition of their own: an
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } => {
```

with:

```rust
            // Diagnostics only, with no transition of their own: an
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing. Pending requests and answer verdicts
            // are stored the same way until the store keeps a pending set.
            SessionBody::AdapterExited { .. }
            | SessionBody::HostNote { .. }
            | SessionBody::PendingOpened { .. }
            | SessionBody::PendingResolved { .. }
            | SessionBody::AnswerResult { .. } => {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        SessionBody::ConfigApplied { .. } => "config_applied",
    }
```

with:

```rust
        SessionBody::ConfigApplied { .. } => "config_applied",
        SessionBody::PendingOpened { .. } => "pending_opened",
        SessionBody::PendingResolved { .. } => "pending_resolved",
        SessionBody::AnswerResult { .. } => "answer_result",
    }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        presumed_parked: session.presumed_parked,
        open_turn,
    })
    .into_response()
```

with:

```rust
        presumed_parked: session.presumed_parked,
        open_turn,
        pending: Vec::new(),
    })
    .into_response()
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

with:

```rust
        // Answers reach the session actor once it keeps its pending requests.
        CollectorFrame::AnswerPermission { request_id, .. } | CollectorFrame::AnswerElicitation { request_id, .. } => {
            uplink.reply(HostFrame::Error {
                request_id,
                code: "unsupported".into(),
                message: "this host does not take answers yet".into(),
            })
        }
        CollectorFrame::Ack { session_id, ack_seq } => uplink.ack(&session_id, ack_seq)?,
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
                SessionBody::ConfigApplied { .. } => "config_applied".to_string(),
            },
```

with:

```rust
                SessionBody::ConfigApplied { .. } => "config_applied".to_string(),
                SessionBody::PendingOpened { indexed, .. } => match &indexed.pending {
                    Some(pending) => format!("pending_opened:{}", tag(pending.kind)),
                    None => "pending_opened:?".to_string(),
                },
                SessionBody::PendingResolved { resolution, reason, .. } => match reason {
                    Some(reason) => format!("pending_resolved:{}:{}", tag(resolution), tag(reason)),
                    None => format!("pending_resolved:{}", tag(resolution)),
                },
                SessionBody::AnswerResult { delivered, .. } => format!("answer_result:{delivered}"),
            },
```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
async fn wait_until(uplink: &Uplink, pred: impl Fn(&[HostFrame]) -> bool) -> Vec<HostFrame> {
```

with:

```rust
/// A wire enum's snake_case name.
fn tag(value: impl serde::Serialize) -> String {
    serde_json::to_value(value).unwrap().as_str().unwrap().to_string()
}

async fn wait_until(uplink: &Uplink, pred: impl Fn(&[HostFrame]) -> bool) -> Vec<HostFrame> {
```

- [ ] **Step 4: Regenerate, then run everything**

Run: `cargo run -p hennery-proto --bin gen`
Expected: `wrote schema/hennery-protocol.schema.json`, `wrote web/src/generated/protocol.ts`.

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 261 tests pass, including `pending_bodies_use_the_spec_field_names`, `answer_frames_use_the_spec_field_names`, `a_rest_answer_is_an_option_or_an_elicitation_action` and `the_session_detail_shows_the_open_turn`. The generated TypeScript declares `export type AnswerRequest = { option_id: string, } | { action: ElicitationAction, content?: unknown, };`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(proto): pending requests, answers and their verdicts"
git push -u origin HEAD
```

### Task 2: The fake adapter asks questions

**Files:**
- Modify: `crates/hennery-testkit/src/lib.rs`, `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`
- Test: `crates/hennery-testkit/tests/fake_acp.rs`

**Interfaces:**
- Consumes: nothing new (the fake speaks raw ACP).
- Produces: `enum FakeAsk { Permission, Elicitation, Unknown, FuturePermission }` (snake_case). Also new `FakeScript` fields:
  - `asks: Vec<FakeAsk>`: asked at the start of every prompt, one at a time;
  - `asks_at_once: bool`: all sent, then awaited in order;
  - `crash_while_asking: bool`: sent, then exit status 3 with no answer awaited;
  - `ask_on_load: bool`: the first ask goes out right before the `session/load` answer;
  - `ask_on_load_waits: bool`: with `ask_on_load`, the load is answered only once that ask is;
  - `withdraw_asks: bool`: every ask is withdrawn with `$/cancel_request` right after it is sent.
- Produces: the requests and their echoes:
  - `Permission` sends `session/request_permission` for `toolCallId: "call-1"`, with options `allow` / `reject`;
  - `Elicitation` sends `elicitation/create` in form mode, asking for a `name`, but only to a client whose `initialize` advertised `elicitation.form`. It parses that typed, so a boolean does not count (P-19);
  - `Unknown` sends `_fake/unknown`;
  - `FuturePermission` is `Permission` with options `allow` and `allow_session` (kind `allow_for_session`, unknown to the schema).

  Each answer is echoed as one `agent_message_chunk`: `permission:selected:<id>` | `permission:cancelled`, `elicitation:accept:<content JSON>` | `elicitation:decline` | `elicitation:cancel` | `elicitation:unsupported`, `<name>:error:<code>`.
- Produces: cancellation. A prompt cancelled while it asks sends nothing more, and ends `cancelled` once its open asks are answered.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/fake_acp.rs`, replace:

```rust
//! The fake adapter speaks ACP over stdio like a real one.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
```

with:

```rust
//! The fake adapter speaks ACP over stdio like a real one.

use hennery_testkit::{FakeAsk, FakeScript};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;
```

Append to `crates/hennery-testkit/tests/fake_acp.rs`:

```rust
// Plan (2): the fake asks the client questions (ACP core §2.5, §4.6).

/// What the fake sent during one `converse`, and how it exited if it did.
struct Conversation {
    messages: Vec<Value>,
    exit: Option<i32>,
}

/// Drive the fake through `initialize` (advertising `capabilities`),
/// `session/new` and one prompt, playing the client: every request the fake
/// sends is handed to `on_request`, and whatever it returns is written back.
/// Ends when the prompt is answered or the fake exits; a watchdog kills a
/// fake still running after 20 s, so a question nobody answers fails the
/// test instead of hanging it.
fn converse(script: &FakeScript, capabilities: Value, on_request: impl FnMut(&Value) -> Vec<Value>) -> Conversation {
    let mut requests = session_requests();
    requests[0]["params"]["clientCapabilities"] = capabilities;
    converse_with(script, requests, on_request)
}

/// `converse` over any list of requests; it ends when the last one is
/// answered.
fn converse_with(
    script: &FakeScript,
    requests: Vec<Value>,
    mut on_request: impl FnMut(&Value) -> Vec<Value>,
) -> Conversation {
    let last = requests.last().unwrap()["id"].clone();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hennery-fake-acp"))
        .env(hennery_testkit::SCRIPT_ENV, serde_json::to_string(script).unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let pid = child.id() as i32;
    let watchdog = std::thread::spawn(move || {
        if finished.recv_timeout(Duration::from_secs(20)).is_err() {
            // SAFETY: the child is not reaped before `done` is sent, so its
            // pid cannot have been recycled yet.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    });
    let mut stdin = child.stdin.take().unwrap();
    for r in &requests {
        writeln!(stdin, "{r}").unwrap();
    }
    let mut messages = Vec::new();
    let mut answered = false;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let msg: Value = serde_json::from_str(&line.unwrap()).unwrap();
        if msg.get("method").is_some() && msg.get("id").is_some() {
            for reply in on_request(&msg) {
                writeln!(stdin, "{reply}").unwrap();
            }
        }
        answered = msg["id"] == last && msg.get("method").is_none();
        messages.push(msg);
        if answered {
            break;
        }
    }
    if answered {
        child.kill().ok();
    }
    let _ = done.send(());
    watchdog.join().unwrap();
    let status = child.wait().unwrap();
    Conversation {
        messages,
        exit: if answered { None } else { status.code() },
    }
}

/// The client's answer to one of the fake's requests.
fn result(request: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
}

/// The text of every `agent_message_chunk`, in order.
fn texts(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["method"] == "session/update")
        .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
        .map(str::to_string)
        .collect()
}

fn asking(asks: Vec<FakeAsk>) -> FakeScript {
    FakeScript {
        asks,
        ..FakeScript::default()
    }
}

#[test]
fn a_permission_ask_waits_for_the_clients_choice_and_the_agent_sees_it() {
    let mut asked = Vec::new();
    let talk = converse(&asking(vec![FakeAsk::Permission]), json!({}), |req| {
        asked.push(req.clone());
        vec![result(
            req,
            json!({"outcome": {"outcome": "selected", "optionId": "allow"}}),
        )]
    });
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["method"], "session/request_permission");
    assert_eq!(asked[0]["params"]["sessionId"], "fake-session-1");
    let options: Vec<&str> = asked[0]["params"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["optionId"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["allow", "reject"]);
    assert_eq!(texts(&talk.messages), ["permission:selected:allow", "Hello", " world"]);
    assert_eq!(talk.messages.last().unwrap()["result"]["stopReason"], "end_turn");
}

#[test]
fn an_elicitation_is_asked_only_of_a_client_that_advertises_form_elicitation() {
    let script = asking(vec![FakeAsk::Elicitation]);
    let talk = converse(&script, json!({"elicitation": {"form": {}}}), |req| {
        assert_eq!(
            (req["method"].as_str(), req["params"]["mode"].as_str()),
            (Some("elicitation/create"), Some("form"))
        );
        vec![result(
            req,
            json!({"action": "accept", "content": {"name": "notes.txt"}}),
        )]
    });
    assert_eq!(texts(&talk.messages)[0], r#"elicitation:accept:{"name":"notes.txt"}"#);
    // A boolean is not the capability (P-19): the agent asks nothing, as
    // if none were advertised.
    for caps in [json!({"elicitation": true}), json!({})] {
        let talk = converse(&script, caps.clone(), |req| {
            panic!("asked {req} of a client with {caps}")
        });
        assert_eq!(texts(&talk.messages)[0], "elicitation:unsupported", "{caps}");
    }
}

#[test]
fn a_prompt_cancelled_while_it_asks_stops_asking_and_ends_cancelled() {
    let mut asked = 0;
    let talk = converse(
        &asking(vec![FakeAsk::Permission, FakeAsk::Elicitation]),
        json!({"elicitation": {"form": {}}}),
        |req| {
            asked += 1;
            // A client cancels the turn, then answers what is pending as
            // cancelled, as ACP asks of it.
            vec![
                json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "fake-session-1"}}),
                result(req, json!({"outcome": {"outcome": "cancelled"}})),
            ]
        },
    );
    assert_eq!(asked, 1, "the agent kept asking after the cancel");
    assert_eq!(texts(&talk.messages), ["permission:cancelled"]);
    assert_eq!(talk.messages.last().unwrap()["result"]["stopReason"], "cancelled");
}

#[test]
fn asks_at_once_are_all_open_before_the_first_answer() {
    let script = FakeScript {
        asks_at_once: true,
        ..asking(vec![FakeAsk::Permission, FakeAsk::Elicitation])
    };
    let mut open = Vec::new();
    let talk = converse(&script, json!({"elicitation": {"form": {}}}), |req| {
        open.push(req.clone());
        if open.len() < 2 {
            return vec![];
        }
        // Answered newest first: the echoes still follow the asks' order.
        vec![
            result(&open[1], json!({"action": "decline"})),
            result(
                &open[0],
                json!({"outcome": {"outcome": "selected", "optionId": "reject"}}),
            ),
        ]
    });
    assert_eq!(open.len(), 2);
    assert_eq!(
        texts(&talk.messages)[..2],
        ["permission:selected:reject", "elicitation:decline"]
    );
}

#[test]
fn a_request_no_client_serves_comes_back_with_the_clients_error() {
    let talk = converse(&asking(vec![FakeAsk::Unknown]), json!({}), |req| {
        assert_eq!(req["method"], "_fake/unknown");
        vec![json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32601, "message": "Method not found"}})]
    });
    assert_eq!(texts(&talk.messages)[0], "unknown:error:-32601");
}

#[test]
fn ask_on_load_asks_before_the_load_is_answered() {
    let script = FakeScript {
        ask_on_load: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let requests = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{"sessionId":"fake-session-1","cwd":"/tmp","mcpServers":[]}}),
    ];
    let talk = converse_with(&script, requests, |_| vec![]);
    let asked = talk
        .messages
        .iter()
        .position(|m| m["method"] == "session/request_permission")
        .expect("asked");
    let loaded = talk.messages.iter().position(|m| m["id"] == json!(2)).unwrap();
    assert!(asked < loaded, "{:?}", talk.messages);
}

#[test]
fn a_withdrawn_ask_is_cancelled_on_the_wire() {
    let script = FakeScript {
        withdraw_asks: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let mut asked = Vec::new();
    let talk = converse(&script, json!({}), |req| {
        asked.push(req.clone());
        vec![json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32800, "message": "Request cancelled"}})]
    });
    let withdrawn = talk
        .messages
        .iter()
        .find(|m| m["method"] == "$/cancel_request")
        .expect("the ask was withdrawn");
    assert_eq!(withdrawn["params"]["requestId"], asked[0]["id"]);
    assert_eq!(texts(&talk.messages)[0], "permission:error:-32800");
}

#[test]
fn crash_while_asking_exits_with_the_question_unanswered() {
    let script = FakeScript {
        crash_while_asking: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let mut asked = 0;
    let talk = converse(&script, json!({}), |_| {
        asked += 1;
        vec![]
    });
    assert_eq!(asked, 1);
    assert_eq!(talk.exit, Some(hennery_testkit::CRASH_EXIT_CODE));
    assert!(texts(&talk.messages).is_empty(), "{:?}", talk.messages);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-testkit --test fake_acp --locked`
Expected: FAIL to compile, with `error[E0432]: unresolved import hennery_testkit::FakeAsk` and `error[E0560]: struct FakeScript has no field named asks`.

- [ ] **Step 3: Let the fake ask**

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_answer_on_file: Option<String>,
}
```

with:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_switch_answer_on_file: Option<String>,
    /// Questions asked at the start of every prompt, before its chunks, in
    /// order, each awaited before the next. Each answer is echoed as an
    /// `agent_message_chunk` (see `FakeAsk`), so a test sees what reached
    /// the agent. A prompt cancelled meanwhile asks nothing more and ends
    /// `cancelled` once its open questions are answered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<FakeAsk>,
    /// Send every ask at once, then await the answers in the asks' order
    /// (several questions open together).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asks_at_once: bool,
    /// Send the asks, then crash (exit status 3) without awaiting their
    /// answers: an adapter lost with its questions open.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crash_while_asking: bool,
    /// Send the first ask right before answering `session/load`, and echo
    /// its answer once it comes, outside any turn: a question that arrives
    /// while the host is still attaching the session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask_on_load: bool,
    /// With `ask_on_load`: answer `session/load` only once that ask is
    /// answered (an adapter that blocks its load on a question).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask_on_load_waits: bool,
    /// Withdraw every ask right after sending it (`$/cancel_request`), as an
    /// agent that no longer needs the answer does. The echo is whatever the
    /// client answers then, usually `<name>:error:-32800`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withdraw_asks: bool,
}

/// One question the fake asks its client during a prompt, and the chunk it
/// echoes the answer as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FakeAsk {
    /// `session/request_permission` for tool call `call-1`, with options
    /// `allow` (`allow_once`) and `reject` (`reject_once`). Echoed as
    /// `permission:selected:<option>` or `permission:cancelled`.
    Permission,
    /// `elicitation/create` in form mode, asking for a `name` string: only
    /// of a client whose `initialize` advertised `elicitation.form`, as the
    /// real adapters do (P-19). Echoed as `elicitation:accept:<content>`,
    /// `elicitation:decline`, `elicitation:cancel`, or, when not asked,
    /// `elicitation:unsupported`.
    Elicitation,
    /// `_fake/unknown`, a method no client serves. Echoed as
    /// `unknown:error:<JSON-RPC code>`.
    Unknown,
    /// Like `Permission`, with a second option `allow_session` of a kind
    /// this build's schema does not know (`allow_for_session`): an adapter
    /// newer than hennery. Echoed like `Permission`.
    FuturePermission,
}
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust
            model_switch_answer_on_file: None,
        }
```

with:

```rust
            model_switch_answer_on_file: None,
            asks: Vec::new(),
            asks_at_once: false,
            crash_while_asking: false,
            ask_on_load: false,
            ask_on_load_waits: false,
            withdraw_asks: false,
        }
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
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
```

with:

```rust
    SessionConfigSelect, SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
    TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, SentRequest, Stdio, UntypedMessage};
use hennery_testkit::{CRASH_EXIT_CODE, FakeAsk, FakeScript, SCRIPT_ENV};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
    let cancel = Arc::new(watch::channel(false).0);
    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            {
                let catalogue = catalogue.clone();
                async move |req: InitializeRequest, responder, _cx| {
```

with:

```rust
    let cancel = Arc::new(watch::channel(false).0);
    // The client advertised form elicitation in `initialize`.
    let forms = Arc::new(AtomicBool::new(false));
    Agent
        .builder()
        .name("hennery-fake-acp")
        .on_receive_request(
            {
                let catalogue = catalogue.clone();
                let forms = forms.clone();
                async move |req: InitializeRequest, responder, _cx| {
                    // Typed, like the real adapters' schema validation: a
                    // boolean `elicitation` does not parse, so it counts as
                    // not advertised (P-19).
                    let advertised = req
                        .client_capabilities
                        .elicitation
                        .as_ref()
                        .is_some_and(|elicitation| elicitation.form.is_some());
                    forms.store(advertised, Ordering::SeqCst);
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
        .on_receive_request(
            {
                let script = script.clone();
                async move |req: PromptRequest, responder, cx| {
```

with:

```rust
        .on_receive_request(
            {
                let script = script.clone();
                let forms = forms.clone();
                async move |req: PromptRequest, responder, cx| {
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                    cx.spawn(async move {
                        if script.flood {
```

with:

```rust
                    let forms = forms.load(Ordering::SeqCst);
                    cx.spawn(async move {
                        if script.crash_while_asking {
                            // Sent, never awaited: kept alive until the crash.
                            let mut sent: Vec<SentRequest<serde_json::Value>> = Vec::new();
                            for ask in &script.asks {
                                if let Some(request) = ask_request(*ask, &req.session_id, forms)? {
                                    sent.push(cx2.send_request(request));
                                }
                            }
                            crash().await;
                        }
                        if !script.asks.is_empty() {
                            for echo in ask_all(&cx2, &script, &req.session_id, forms, &cancelled).await? {
                                cx2.send_notification(chunk(&req.session_id, echo))?;
                            }
                            if *cancelled.borrow() {
                                return responder.respond(PromptResponse::new(StopReason::Cancelled));
                            }
                        }
                        if script.flood {
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
/// Record a switch in the script's `config_log`, if it has one.
```

with:

```rust
/// One text chunk of the agent's reply.
fn chunk(session: &SessionId, text: String) -> SessionNotification {
    SessionNotification::new(
        session.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(text)))),
    )
}

/// The ACP request for one ask, sent untyped so the test sees the answer
/// exactly as the client wrote it. `None` for an elicitation to a client
/// that did not advertise form elicitation.
fn ask_request(
    ask: FakeAsk,
    session: &SessionId,
    forms: bool,
) -> agent_client_protocol::Result<Option<UntypedMessage>> {
    match ask {
        FakeAsk::Permission => UntypedMessage::new(
            "session/request_permission",
            serde_json::json!({
                "sessionId": session,
                "toolCall": {"toolCallId": "call-1", "title": "Write notes.txt", "kind": "edit"},
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
                ]
            }),
        )
        .map(Some),
        FakeAsk::FuturePermission => UntypedMessage::new(
            "session/request_permission",
            serde_json::json!({
                "sessionId": session,
                "toolCall": {"toolCallId": "call-1", "title": "Write notes.txt", "kind": "edit"},
                "options": [
                    {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                    {"optionId": "allow_session", "name": "Allow for this session", "kind": "allow_for_session"}
                ]
            }),
        )
        .map(Some),
        FakeAsk::Elicitation if forms => UntypedMessage::new(
            "elicitation/create",
            serde_json::json!({
                "mode": "form",
                "sessionId": session,
                "message": "What should the file be called?",
                "requestedSchema": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }
            }),
        )
        .map(Some),
        FakeAsk::Elicitation => Ok(None),
        FakeAsk::Unknown => UntypedMessage::new("_fake/unknown", serde_json::json!({ "sessionId": session })).map(Some),
    }
}

/// An ask whose echo is known (not asked), or still out.
enum Asked {
    Echo(String),
    Out(FakeAsk, SentRequest<serde_json::Value>),
}

/// Ask the script's questions and return one echo per ask, in order. One
/// at a time, unless `asks_at_once`; a cancelled prompt asks nothing more.
async fn ask_all(
    cx: &ConnectionTo<Client>,
    script: &FakeScript,
    session: &SessionId,
    forms: bool,
    cancelled: &watch::Receiver<bool>,
) -> agent_client_protocol::Result<Vec<String>> {
    let mut asked = Vec::new();
    for ask in &script.asks {
        if *cancelled.borrow() {
            break;
        }
        let Some(request) = ask_request(*ask, session, forms)? else {
            asked.push(Asked::Echo("elicitation:unsupported".into()));
            continue;
        };
        let sent = cx.send_request(request);
        if script.withdraw_asks {
            sent.cancel()?;
        }
        asked.push(if script.asks_at_once {
            Asked::Out(*ask, sent)
        } else {
            Asked::Echo(echo(*ask, sent.block_task().await))
        });
    }
    let mut echoes = Vec::new();
    for asked in asked {
        echoes.push(match asked {
            Asked::Echo(echo) => echo,
            Asked::Out(ask, sent) => echo(ask, sent.block_task().await),
        });
    }
    Ok(echoes)
}

/// The answer to one ask, as the agent understood it.
fn echo(ask: FakeAsk, answer: agent_client_protocol::Result<serde_json::Value>) -> String {
    let name = match ask {
        FakeAsk::Permission | FakeAsk::FuturePermission => "permission",
        FakeAsk::Elicitation => "elicitation",
        FakeAsk::Unknown => "unknown",
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(err) => return format!("{name}:error:{}", i32::from(err.code)),
    };
    match ask {
        FakeAsk::Permission | FakeAsk::FuturePermission => match answer["outcome"]["outcome"].as_str() {
            Some("selected") => format!(
                "permission:selected:{}",
                answer["outcome"]["optionId"].as_str().unwrap_or("?")
            ),
            Some(other) => format!("permission:{other}"),
            None => format!("permission:unreadable:{answer}"),
        },
        FakeAsk::Elicitation => match (answer["action"].as_str(), answer.get("content")) {
            (Some(action), Some(content)) => format!("elicitation:{action}:{content}"),
            (Some(action), None) => format!("elicitation:{action}"),
            (None, _) => format!("elicitation:unreadable:{answer}"),
        },
        FakeAsk::Unknown => format!("unknown:answered:{answer}"),
    }
}

/// Record a switch in the script's `config_log`, if it has one.
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                let script = script.clone();
                let announced = announced.clone();
                async move |req: LoadSessionRequest, responder, cx| {
```

with:

```rust
                let script = script.clone();
                let announced = announced.clone();
                let forms = forms.clone();
                async move |req: LoadSessionRequest, responder, cx| {
```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

```rust
                    match script.load_error {
                        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
```

with:

```rust
                    if script.ask_on_load
                        && let Some(ask) = script.asks.first().copied()
                        && let Some(request) = ask_request(ask, &req.session_id, forms.load(Ordering::SeqCst))?
                    {
                        // On the wire before the load's answer; awaited
                        // from a task of its own.
                        let sent = cx.send_request(request);
                        if script.ask_on_load_waits {
                            // The load is answered only once the question is.
                            return cx.spawn(async move {
                                let _ = sent.block_task().await;
                                responder.respond(LoadSessionResponse::new())
                            });
                        }
                        let (cx2, session) = (cx.clone(), req.session_id.clone());
                        cx.spawn(async move {
                            let echo = echo(ask, sent.block_task().await);
                            cx2.send_notification(chunk(&session, echo))
                        })?;
                    }
                    match script.load_error {
                        Some(code) => responder.respond_with_error(agent_client_protocol::Error::new(code, "scripted")),
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all 269 tests pass, including the eight new `fake_acp` tests. The watchdog in `converse` means a question nobody answers fails its test after 20 s instead of hanging it.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test(testkit): the fake adapter asks permission and elicitation questions"
git push
```

### Task 3: The host forwards questions and delivers answers

**Files:**
- Modify: `crates/hennery-host/Cargo.toml`, `Cargo.lock` (`uuid`), `crates/hennery-host/src/session.rs`, `crates/hennery-host/src/connection.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`, `crates/hennery-testkit/tests/host_connection.rs`, and the unit tests in `crates/hennery-host/src/session.rs`

**Interfaces:**
- Consumes (Task 1): `PendingKind`, `PendingExtract`, `PendingResolution`, `PendingReason`, `ElicitationAction`, `SessionBody::{PendingOpened, PendingResolved, AnswerResult}`, `CollectorFrame::{AnswerPermission, AnswerElicitation}`.
- Consumes (Task 2): `FakeAsk`, `FakeScript::{asks, asks_at_once, ask_on_load}`.
- Produces: `pub enum hennery_host::session::Answer { Permission { option_id: String }, Elicitation { action: ElicitationAction, content: Option<Value> } }`, and `SessionCmd::Answer { request_id: String, pending_id: String, answer: Answer }`.
- Produces: `initialize` advertises `clientCapabilities.elicitation = {"form": {}}`, next to B2b's boolean config options (`fn client_capabilities() -> ClientCapabilities`).
- Produces: the untyped request handler:
  - `session/request_permission` and `elicitation/create` become `Inbound::Question(Box<Question>)`, on the same ordered channel as the notifications;
  - anything else is answered `-32601` right there (decision 3).
- Produces: `fn option_ids(params: &Value) -> Option<Vec<String>>`, read from the raw request (decision 4).
- Produces: `Actor::open_question`:
  - it mints a `pending_id` (UUIDv7) and emits `pending_opened`, with `indexed.turn_id` and `indexed.pending`;
  - it keeps the responder in `Questions::open`;
  - questions that arrive before `session_started` are held in `Early` and opened right after it, in wire order (decision 2);
  - `negotiate` fills a `Replay` owned by `drive`, so a start that runs out of time can name what it held back (`fn held_questions`).
- Produces: `Actor::answer`. A held question gets `answer_result{true}`, then `pending_resolved{delivered}`. Anything else gets `answer_result{false}`.
- Produces: the connection routes `answer_*` to the live actor. With none, it answers `error{not_attached}` (decision 7).

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/host_session.rs`, replace:

```rust
use hennery_host::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, SessionBody, SessionConfig, TurnOutcome};
use hennery_testkit::{FakeScript, SCRIPT_ENV, pid_alive};
```

with:

```rust
use hennery_host::session::{self, AgentCommand, Answer, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
use hennery_host::uplink::Uplink;
use hennery_proto::frames::{
    ConfigValue, ElicitationAction, HostFrame, Indexed, PendingExtract, PendingKind, SessionBody, SessionConfig,
    TurnOutcome,
};
use hennery_testkit::{FakeAsk, FakeScript, SCRIPT_ENV, pid_alive};
```

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
// Plan (2): permission and elicitation (ACP core §2.5, §4.6).

fn asking(asks: Vec<FakeAsk>) -> FakeScript {
    FakeScript {
        asks,
        ..FakeScript::default()
    }
}

/// A new session `s1` of the fake with `script`, default options.
fn starting(uplink: &Uplink, script: &FakeScript) -> SessionHandle {
    session::start(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        fake_with(script),
        std::env::temp_dir(),
    )
}

/// Every `pending_opened`: its extract, turn and ACP payload.
fn opened(frames: &[HostFrame]) -> Vec<(PendingExtract, Option<String>, serde_json::Value)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body: SessionBody::PendingOpened { indexed, payload, .. },
                ..
            } => Some((
                indexed.pending.clone().unwrap(),
                indexed.turn_id.clone(),
                payload.clone(),
            )),
            _ => None,
        })
        .collect()
}

/// The id of the `n`th pending request opened, once it is.
async fn nth_pending(uplink: &Uplink, n: usize) -> String {
    let frames = wait_until(uplink, |f| opened(f).len() > n).await;
    opened(&frames)[n].0.id.clone()
}

fn choose(request_id: &str, pending_id: &str, option_id: &str) -> SessionCmd {
    SessionCmd::Answer {
        request_id: request_id.into(),
        pending_id: pending_id.into(),
        answer: Answer::Permission {
            option_id: option_id.into(),
        },
    }
}

/// Every `answer_result`: (request id, pending id, delivered).
fn verdicts(frames: &[HostFrame]) -> Vec<(String, String, bool)> {
    frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::Session {
                body:
                    SessionBody::AnswerResult {
                        pending_id,
                        request_id,
                        delivered,
                    },
                ..
            } => Some((request_id.clone(), pending_id.clone(), *delivered)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_permission_request_waits_for_the_operator_and_the_answer_reaches_the_agent() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = starting(&uplink, &asking(vec![FakeAsk::Permission]));
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let pending = nth_pending(&uplink, 0).await;
    // Nothing moves until the operator answers: the turn stays open.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let frames = uplink.pending().unwrap();
    assert_eq!(
        kinds(&frames),
        ["session_started", "turn_started", "pending_opened:permission"]
    );
    assert_eq!(handle.open_turn_id().as_deref(), Some("t1"));
    let (extract, turn, payload) = opened(&frames).remove(0);
    assert_eq!(
        (extract.kind, extract.option_ids, turn.as_deref()),
        (
            PendingKind::Permission,
            Some(vec!["allow".to_string(), "reject".to_string()]),
            Some("t1")
        )
    );
    // Verbatim, `_meta` and all.
    assert_eq!(payload["toolCall"]["toolCallId"], "call-1");

    assert!(handle.send(choose("ra", &pending, "allow")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(
        kinds(&frames)[3..],
        [
            "answer_result:true",
            "pending_resolved:delivered",
            "update:permission:selected:allow",
            "update:Hello",
            "update: world",
            "turn_ended"
        ]
    );
    assert_eq!(verdicts(&frames), [("ra".to_string(), pending, true)]);
}

#[tokio::test]
async fn an_answer_is_delivered_once_and_one_for_a_question_not_open_is_not() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec![],
        ..asking(vec![FakeAsk::Permission])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let pending = nth_pending(&uplink, 0).await;
    assert!(handle.send(choose("ra", &pending, "reject")));
    // A resent answer (a reconnect, a second client) and one for a question
    // this actor never asked: neither has anyone waiting for it.
    assert!(handle.send(choose("rb", &pending, "allow")));
    assert!(handle.send(choose("rc", "no-such-question", "allow")));
    let frames = wait_until(&uplink, |f| verdicts(f).len() == 3).await;
    assert_eq!(
        verdicts(&frames),
        [
            ("ra".to_string(), pending.clone(), true),
            ("rb".to_string(), pending, false),
            ("rc".to_string(), "no-such-question".to_string(), false),
        ]
    );
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let resolved = kinds(&frames)
        .iter()
        .filter(|k| k.starts_with("pending_resolved"))
        .count();
    assert_eq!(resolved, 1, "{:?}", kinds(&frames));
    assert!(kinds(&frames).contains(&"update:permission:selected:reject".to_string()));
}

#[tokio::test]
async fn an_elicitation_reaches_the_operator_and_the_form_content_reaches_the_agent() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    // The fake asks only a client that advertised form elicitation (P-19).
    let handle = starting(&uplink, &asking(vec![FakeAsk::Elicitation]));
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let pending = nth_pending(&uplink, 0).await;
    let frames = uplink.pending().unwrap();
    let (extract, _, payload) = opened(&frames).remove(0);
    assert_eq!((extract.kind, extract.option_ids), (PendingKind::Elicitation, None));
    assert_eq!(payload["mode"], "form");
    assert!(handle.send(SessionCmd::Answer {
        request_id: "ra".into(),
        pending_id: pending,
        answer: Answer::Elicitation {
            action: ElicitationAction::Accept,
            content: Some(json!({"name": "notes.txt"})),
        },
    }));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert!(
        kinds(&frames).contains(&r#"update:elicitation:accept:{"name":"notes.txt"}"#.to_string()),
        "{:?}",
        kinds(&frames)
    );
}

#[tokio::test]
async fn several_open_questions_each_take_their_own_answer() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        asks_at_once: true,
        chunks: vec![],
        ..asking(vec![FakeAsk::Permission, FakeAsk::Elicitation])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let first = nth_pending(&uplink, 0).await;
    let second = nth_pending(&uplink, 1).await;
    assert!(handle.send(SessionCmd::Answer {
        request_id: "rb".into(),
        pending_id: second,
        answer: Answer::Elicitation {
            action: ElicitationAction::Decline,
            content: None,
        },
    }));
    assert!(handle.send(choose("ra", &first, "allow")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    let echoes: Vec<String> = kinds(&frames)
        .into_iter()
        .filter(|k| k.starts_with("update:"))
        .collect();
    assert_eq!(
        echoes,
        ["update:permission:selected:allow", "update:elicitation:decline"]
    );
    assert_eq!(verdicts(&frames).iter().filter(|v| v.2).count(), 2);
}

#[tokio::test]
async fn a_request_the_host_does_not_serve_is_refused_method_not_found() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec![],
        ..asking(vec![FakeAsk::Unknown])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(
        kinds(&frames),
        [
            "session_started",
            "turn_started",
            "update:unknown:error:-32601",
            "turn_ended"
        ]
    );
}

#[tokio::test]
async fn a_question_asked_while_the_session_loads_opens_after_session_started() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        ask_on_load: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let handle = resuming(&uplink, &script);
    let pending = nth_pending(&uplink, 0).await;
    let frames = uplink.pending().unwrap();
    assert_eq!(kinds(&frames), ["session_started", "pending_opened:permission"]);
    assert_eq!(opened(&frames)[0].1, None, "asked outside any turn");
    assert!(handle.send(choose("ra", &pending, "allow")));
    wait_until(&uplink, has("update:permission:selected:allow")).await;
}

/// A newer adapter may offer an option of a kind this build's schema does
/// not know: the question keeps every option id, and each can be chosen.
#[tokio::test]
async fn a_permission_with_an_option_kind_this_build_does_not_know_keeps_its_option_ids() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        chunks: vec![],
        ..asking(vec![FakeAsk::FuturePermission])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let pending = nth_pending(&uplink, 0).await;
    let frames = uplink.pending().unwrap();
    assert_eq!(
        opened(&frames)[0].0.option_ids,
        Some(vec!["allow".to_string(), "allow_session".to_string()])
    );
    assert!(handle.send(choose("ra", &pending, "allow_session")));
    wait_until(&uplink, has("update:permission:selected:allow_session")).await;
}

/// A question held back during the load, which the load then waits for:
/// the start runs out of time, and its failure says why.
#[tokio::test]
async fn a_start_that_runs_out_of_time_says_which_questions_it_held_back() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        ask_on_load: true,
        ask_on_load_waits: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let _handle = session::resume(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        "agent-7".into(),
        fake_with(&script),
        std::env::temp_dir(),
        SessionOptions {
            start_timeout: Duration::from_secs(3),
            ..SessionOptions::default()
        },
    );
    let frames = wait_until(&uplink, has("start_failed")).await;
    let HostFrame::Session {
        body: SessionBody::StartFailed { message, .. },
        ..
    } = &frames[0]
    else {
        panic!("{:?}", kinds(&frames));
    };
    assert!(
        message.contains(
            "the agent asked 1 question(s) during start-up (permission) that hennery cannot show before the session exists"
        ),
        "{message}"
    );
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
#[cfg(test)]
mod tests {
    use super::*;
```

with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A boolean would be discarded by the adapter's schema validator,
    /// which looks exactly like not advertising it (P-19).
    #[test]
    fn form_elicitation_is_advertised_as_an_object() {
        let advertised = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(advertised["elicitation"], serde_json::json!({"form": {}}));
    }

    /// Read raw: one option of a kind this build does not know must not
    /// cost the whole list.
    #[test]
    fn option_ids_are_read_from_the_raw_request() {
        let options = serde_json::json!({"options": [
            {"optionId": "a", "kind": "from_the_future"}, {"name": "no id"}, {"optionId": 3}, {"optionId": "b"}
        ]});
        assert_eq!(option_ids(&options), Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(option_ids(&serde_json::json!({})), None);
        assert_eq!(option_ids(&serde_json::json!({"options": {"optionId": "a"}})), None);
    }
```

Append to `crates/hennery-testkit/tests/host_connection.rs`:

```rust
// Plan (2): answers over the connection.

fn asking_fake() -> hennery_host::AgentCommand {
    let mut fake = hennery_host::AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    let script = hennery_testkit::FakeScript {
        asks: vec![hennery_testkit::FakeAsk::Permission],
        ..Default::default()
    };
    fake.env.push((
        hennery_testkit::SCRIPT_ENV.into(),
        serde_json::to_string(&script).unwrap(),
    ));
    fake
}

/// A session frame's body as JSON.
fn body_of(frame: &HostFrame) -> serde_json::Value {
    match frame {
        HostFrame::Session { body, .. } => serde_json::to_value(body).unwrap(),
        other => panic!("expected a session frame, got {other:?}"),
    }
}

#[tokio::test]
async fn answers_reach_the_actor_and_one_for_a_detached_session_is_not_attached() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "answers", asking_fake())));

    let (mut sink, mut stream, _) = accept_host(&listener).await;
    send_frame(&mut sink, &start("r1", "s1")).await;
    read_until(&mut stream, body_is("s1", "session_started")).await;
    let prompt = CollectorFrame::Prompt {
        request_id: "r2".into(),
        session_id: "s1".into(),
        turn_id: "t1".into(),
        content: vec![serde_json::json!({"type": "text", "text": "hi"})],
    };
    send_frame(&mut sink, &prompt).await;
    let opened = body_of(&read_until(&mut stream, body_is("s1", "pending_opened")).await);
    let pending_id = opened["pending_id"].as_str().unwrap().to_string();
    let answer = |request_id: &str, session_id: &str| CollectorFrame::AnswerPermission {
        request_id: request_id.into(),
        session_id: session_id.into(),
        pending_id: pending_id.clone(),
        option_id: "allow".into(),
    };
    send_frame(&mut sink, &answer("r3", "s1")).await;
    let result = body_of(&read_until(&mut stream, body_is("s1", "answer_result")).await);
    assert_eq!(
        (&result["request_id"], &result["delivered"]),
        (&serde_json::json!("r3"), &serde_json::json!(true))
    );
    send_frame(&mut sink, &answer("r4", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session --locked`
Expected: FAIL to compile, with `error[E0432]: unresolved import hennery_host::session::Answer` and `error[E0599]: no variant named Answer found for enum SessionCmd`.

- [ ] **Step 3: Forward the questions, hold them until the start is announced, deliver the answers**

In `crates/hennery-host/Cargo.toml`, replace:

```toml
tracing.workspace = true

[dev-dependencies]
```

with:

```toml
tracing.workspace = true
uuid.workspace = true

[dev-dependencies]
```

In `Cargo.lock`, replace:

```
 "tokio-tungstenite",
 "tokio-util",
 "tracing",
]

[[package]]
name = "hennery-kernel"
```

with:

```
 "tokio-tungstenite",
 "tokio-util",
 "tracing",
 "uuid",
]

[[package]]
name = "hennery-kernel"
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
use agent_client_protocol::schema::v1::{
    BooleanConfigOptionCapabilities, CancelNotification, ClientCapabilities, ClientSessionCapabilities, ContentBlock,
    InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest, PromptResponse, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigOptionsCapabilities,
    SessionId, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
    StopReason,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, UntypedMessage};
use hennery_proto::frames::{ConfigValue, HostFrame, Indexed, ParkReason, SessionBody, SessionConfig, TurnOutcome};
```

with:

```rust
use agent_client_protocol::schema::v1::{
    BooleanConfigOptionCapabilities, CancelNotification, ClientCapabilities, ClientSessionCapabilities, ContentBlock,
    ElicitationCapabilities, ElicitationFormCapabilities, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, PromptResponse, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigOptionValue, SessionConfigOptionsCapabilities, SessionId, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, ErrorCode, Responder, UntypedMessage};
use hennery_proto::frames::{
    ConfigValue, ElicitationAction, HostFrame, Indexed, ParkReason, PendingExtract, PendingKind, PendingReason,
    PendingResolution, SessionBody, SessionConfig, TurnOutcome,
};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    SetConfig {
        request_id: String,
        config_id: String,
        value: ConfigValue,
    },
}
```

with:

```rust
    SetConfig {
        request_id: String,
        config_id: String,
        value: ConfigValue,
    },
    /// The operator's answer to one of the adapter's questions: answered by
    /// `answer_result` (ACP core §4.6).
    Answer {
        request_id: String,
        pending_id: String,
        answer: Answer,
    },
}

/// An operator's answer to a pending request, as the connection hands it on.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// One of a permission request's options.
    Permission { option_id: String },
    /// An elicitation's action; `content` only when it accepts.
    Elicitation {
        action: ElicitationAction,
        content: Option<Value>,
    },
}

impl Answer {
    fn kind(&self) -> PendingKind {
        match self {
            Answer::Permission { .. } => PendingKind::Permission,
            Answer::Elicitation { .. } => PendingKind::Elicitation,
        }
    }

    /// The ACP response to the adapter's request.
    fn response(&self) -> Value {
        match self {
            Answer::Permission { option_id } => {
                serde_json::json!({"outcome": {"outcome": "selected", "optionId": option_id}})
            }
            Answer::Elicitation { action, content } => {
                let mut response = serde_json::json!({ "action": action });
                if let (ElicitationAction::Accept, Some(content)) = (action, content) {
                    response["content"] = content.clone();
                }
                response
            }
        }
    }
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        catalogue: Mutex::new(Catalogue::default()),
        ending: ending.clone(),
    };
```

with:

```rust
        catalogue: Mutex::new(Catalogue::default()),
        questions: Mutex::new(Questions::default()),
        ending: ending.clone(),
    };
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
enum Inbound {
    Update(Value),
    SwitchAnswer {
        token: u64,
        result: agent_client_protocol::Result<SetSessionConfigOptionResponse>,
    },
}
```

with:

```rust
enum Inbound {
    Update(Value),
    SwitchAnswer {
        token: u64,
        result: agent_client_protocol::Result<SetSessionConfigOptionResponse>,
    },
    /// A question for the operator, in wire order with the notifications
    /// around it: it follows the tool call it asks about.
    Question(Box<Question>),
}

/// A `session/request_permission` or `elicitation/create` from the adapter
/// (ACP core §2.5). Its responder must be kept until the question is
/// answered or cancelled: a dropped responder sends nothing, and the adapter
/// would wait for good.
struct Question {
    kind: PendingKind,
    params: Value,
    responder: Responder<Value>,
}

/// The kind of question an adapter request is, if it is one.
fn question_kind(method: &str) -> Option<PendingKind> {
    match method {
        "session/request_permission" => Some(PendingKind::Permission),
        "elicitation/create" => Some(PendingKind::Elicitation),
        _ => None,
    }
}

/// A permission request's option ids, read from its raw params (ACP core
/// §3.2): every string `options[i].optionId`, skipping an entry without
/// one. A typed parse would fail on a single option of a kind this build
/// does not know (`PermissionOptionKind` has no catch-all), and then no
/// answer could be validated. `None` only if `options` is missing or not
/// an array.
fn option_ids(params: &Value) -> Option<Vec<String>> {
    let options = params.get("options")?.as_array()?;
    Some(
        options
            .iter()
            .filter_map(|option| option.get("optionId")?.as_str().map(str::to_string))
            .collect(),
    )
}

/// The adapter's questions waiting for the operator (ACP core §4.6), oldest
/// first. No timeout: each waits until it is answered or cancelled.
#[derive(Default)]
struct Questions {
    open: Vec<OpenQuestion>,
}

struct OpenQuestion {
    pending_id: String,
    kind: PendingKind,
    responder: Responder<Value>,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// What a `session/load` replayed: state updates to pass through, and the
/// unknown kinds that were dropped (ACP core §4.5).
#[derive(Default)]
struct Replay {
    kept: Vec<Value>,
    unknown: BTreeMap<String, usize>,
}

impl Replay {
    fn observe(&mut self, payload: Value) {
        let kind = payload["update"]["sessionUpdate"]
            .as_str()
            .unwrap_or("<none>")
            .to_string();
        if STATE_KINDS.contains(&kind.as_str()) {
            self.kept.push(payload);
        } else if !HISTORY_KINDS.contains(&kind.as_str()) {
            *self.unknown.entry(kind).or_default() += 1;
        }
    }
```

with:

```rust
/// What the adapter sent before `session_started`, in wire order: emitted
/// right after it.
enum Early {
    Update(Value),
    /// Opened once the session is announced, never before (the collector
    /// has no session to attach it to yet).
    Question(Box<Question>),
}

/// What a `session/load` replayed: state updates to pass through, questions
/// to open, and the unknown kinds that were dropped (ACP core §4.5).
#[derive(Default)]
struct Replay {
    kept: Vec<Early>,
    unknown: BTreeMap<String, usize>,
}

impl Replay {
    fn observe(&mut self, payload: Value) {
        let kind = payload["update"]["sessionUpdate"]
            .as_str()
            .unwrap_or("<none>")
            .to_string();
        if STATE_KINDS.contains(&kind.as_str()) {
            self.kept.push(Early::Update(payload));
        } else if !HISTORY_KINDS.contains(&kind.as_str()) {
            *self.unknown.entry(kind).or_default() += 1;
        }
    }

    /// Whatever the adapter sent while the session was being attached: an
    /// update is replay (`observe`), a question is live and kept.
    fn inbound(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Update(payload) => self.observe(payload),
            Inbound::Question(question) => self.kept.push(Early::Question(question)),
            // No switch is sent before the actor's main loop starts.
            Inbound::SwitchAnswer { .. } => {}
        }
    }

    /// The updates kept, in order.
    fn updates(&self) -> impl DoubleEndedIterator<Item = &Value> {
        self.kept.iter().filter_map(|early| match early {
            Early::Update(payload) => Some(payload),
            Early::Question(_) => None,
        })
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The adapter's config options as last reported.
    catalogue: Mutex<Catalogue>,
    /// Shared with the handle (`SessionHandle::is_ending`).
    ending: Arc<AtomicBool>,
}
```

with:

```rust
    /// The adapter's config options as last reported.
    catalogue: Mutex<Catalogue>,
    /// The adapter's questions waiting for the operator.
    questions: Mutex<Questions>,
    /// Shared with the handle (`SessionHandle::is_ending`).
    ending: Arc<AtomicBool>,
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Restart { request_id }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id }
                | SessionCmd::Cancel { request_id, .. }
                | SessionCmd::SetConfig { request_id, .. } => {
```

with:

```rust
                SessionCmd::Prompt { request_id, .. }
                | SessionCmd::Restart { request_id }
                | SessionCmd::Park { request_id }
                | SessionCmd::Close { request_id }
                | SessionCmd::Cancel { request_id, .. }
                | SessionCmd::SetConfig { request_id, .. }
                | SessionCmd::Answer { request_id, .. } => {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        // Kept for `send_next_switch`'s own `on_receiving_result` callbacks
        // (fix round 2): the notification handler below moves its own clone
        // into the connection task.
        let switch_tx = updates_tx.clone();
```

with:

```rust
        // Kept for `send_next_switch`'s own `on_receiving_result` callbacks
        // (fix round 2): the notification handler below moves its own clone
        // into the connection task.
        let switch_tx = updates_tx.clone();
        let questions_tx = updates_tx.clone();
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
```

with:

```rust
                    agent_client_protocol::on_receive_notification!(),
                )
                // Questions for the operator join the same ordered channel.
                // Every other request is refused `-32601 Method not found`
                // here (ACP core §2.5): left unhandled, the crate would hold
                // any request that names a session (`fs/*`, `terminal/*`)
                // for a session handler that never comes, and the adapter
                // would wait for good.
                .on_receive_request(
                    async move |msg: UntypedMessage, responder: Responder<Value>, _cx| match question_kind(&msg.method)
                    {
                        Some(kind) => {
                            let question = Question {
                                kind,
                                params: msg.params,
                                responder,
                            };
                            let _ = questions_tx.send(Inbound::Question(Box::new(question)));
                            Ok(())
                        }
                        None => responder
                            .respond_with_error(agent_client_protocol::Error::method_not_found().data(msg.method)),
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let (agent_session, replay, applied) = match started {
```

with:

```rust
        let (agent_session, applied) = match started {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        // A load's state updates follow the start they belong to, and so do
        // updates the adapter sent while the start's switches ran. They are
        // older than the catalogue just announced, so they carry no
        // catalogue extracts (P-13). Then the note about what the load
        // dropped (ACP core §4.5), then the one about switches that did not
        // take.
        let mut early = replay.kept.clone();
        // Only what is queued now: a flooding adapter must not hold up the
        // start.
        for _ in 0..updates.len() {
            // A `SwitchAnswer` here is impossible: `send_next_switch` is
            // only ever called from the main loop below, which has not
            // started yet.
            match updates.try_recv() {
                Ok(Inbound::Update(payload)) => early.push(payload),
                Ok(Inbound::SwitchAnswer { .. }) => {}
                Err(_) => break,
            }
        }
        for payload in early {
            self.emit(update(payload, None));
        }
```

with:

```rust
        // A load's state updates follow the start they belong to, and so do
        // updates the adapter sent while the start's switches ran. They are
        // older than the catalogue just announced, so they carry no
        // catalogue extracts (P-13). Questions asked meanwhile open here, in
        // wire order. Then the note about what the load dropped (ACP core
        // §4.5), then the one about switches that did not take.
        let mut early = std::mem::take(&mut replay.kept);
        // Only what is queued now: a flooding adapter must not hold up the
        // start.
        for _ in 0..updates.len() {
            // A `SwitchAnswer` here is impossible: `send_next_switch` is
            // only ever called from the main loop below, which has not
            // started yet.
            match updates.try_recv() {
                Ok(Inbound::Update(payload)) => early.push(Early::Update(payload)),
                Ok(Inbound::Question(question)) => early.push(Early::Question(question)),
                Ok(Inbound::SwitchAnswer { .. }) => {}
                Err(_) => break,
            }
        }
        for early in early {
            match early {
                Early::Update(payload) => self.emit(update(payload, None)),
                Early::Question(question) => self.open_question(question, None),
            }
        }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    Some(SessionCmd::Park { .. }) => {
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs).await;
```

with:

```rust
                    Some(SessionCmd::Answer { request_id, pending_id, answer }) => {
                        idle_since = Instant::now();
                        self.answer(request_id, pending_id, answer);
                    }
                    Some(SessionCmd::Park { .. }) => {
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs).await;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    fn handle_inbound(&self, inbound: Inbound, turn: Option<&str>, configs: &mut PendingConfigs) -> bool {
        match inbound {
            Inbound::Update(payload) => {
                self.emit(self.live_update(payload, turn));
                false
            }
            Inbound::SwitchAnswer { token, result } => self.route_switch_answer(token, result, configs),
        }
    }
```

with:

```rust
    fn handle_inbound(&self, inbound: Inbound, turn: Option<&str>, configs: &mut PendingConfigs) -> bool {
        match inbound {
            Inbound::Update(payload) => {
                self.emit(self.live_update(payload, turn));
                false
            }
            Inbound::SwitchAnswer { token, result } => self.route_switch_answer(token, result, configs),
            Inbound::Question(question) => {
                self.open_question(question, turn);
                false
            }
        }
    }

    /// Announce an adapter's question as `pending_opened` and keep its
    /// responder until the operator answers (ACP core §4.6). `turn` is the
    /// turn it was asked in. A permission's option ids are read from the
    /// raw request (`option_ids`).
    fn open_question(&self, question: Box<Question>, turn: Option<&str>) {
        let Question {
            kind,
            params,
            responder,
        } = *question;
        let pending_id = uuid::Uuid::now_v7().to_string();
        let option_ids = match kind {
            PendingKind::Permission => option_ids(&params),
            PendingKind::Elicitation => None,
        };
        self.emit(SessionBody::PendingOpened {
            pending_id: pending_id.clone(),
            indexed: Indexed {
                turn_id: turn.map(str::to_string),
                pending: Some(PendingExtract {
                    id: pending_id.clone(),
                    kind,
                    option_ids,
                }),
                ..Indexed::default()
            },
            payload: params,
        });
        self.questions.lock().expect("questions lock").open.push(OpenQuestion {
            pending_id,
            kind,
            responder,
        });
    }

    /// Deliver an operator's answer if its question is still open here
    /// (ACP core §4.6): `answer_result{delivered: true}`, then
    /// `pending_resolved{delivered}`. An answer nobody waits for (already
    /// answered, cancelled, asked of an earlier adapter, or of another kind)
    /// is `answer_result{delivered: false}` and changes nothing.
    fn answer(&self, request_id: String, pending_id: String, answer: Answer) {
        let question = {
            let mut questions = self.questions.lock().expect("questions lock");
            let at = questions
                .open
                .iter()
                .position(|q| q.pending_id == pending_id && q.kind == answer.kind());
            at.map(|at| questions.open.remove(at))
        };
        let Some(question) = question else {
            return self.emit(SessionBody::AnswerResult {
                pending_id,
                request_id,
                delivered: false,
            });
        };
        let sent = question.responder.respond(answer.response());
        self.emit(SessionBody::AnswerResult {
            pending_id: pending_id.clone(),
            request_id,
            delivered: sent.is_ok(),
        });
        let (resolution, reason) = match sent {
            Ok(()) => (PendingResolution::Delivered, None),
            // The connection to the adapter is gone: so is the question.
            Err(err) => {
                tracing::warn!(session_id = %self.session_id, error = %err, "answer not sent to the adapter");
                (PendingResolution::Cancelled, Some(PendingReason::AdapterLost))
            }
        };
        self.emit(SessionBody::PendingResolved {
            pending_id,
            resolution,
            reason,
        });
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    // Advertised so that agents offer boolean options as booleans, not as
    // on/off selects (ACP `session.configOptions.boolean`).
    let capabilities = ClientCapabilities::new().session(
        ClientSessionCapabilities::new()
            .config_options(SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new())),
    );
    let init = conn
        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(capabilities))
```

with:

```rust
    let init = conn
        .send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(client_capabilities()))
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            let mut replay = Replay::default();
            for _ in 0..updates.len() {
                match updates.try_recv() {
                    Ok(Inbound::Update(payload)) => replay.kept.push(payload),
                    Ok(Inbound::SwitchAnswer { .. }) => {}
                    Err(_) => break,
                }
            }
            let catalogue = announced_options(created.config_options, &replay.kept);
```

with:

```rust
            for _ in 0..updates.len() {
                match updates.try_recv() {
                    Ok(Inbound::Update(payload)) => replay.kept.push(Early::Update(payload)),
                    Ok(Inbound::Question(question)) => replay.kept.push(Early::Question(question)),
                    Ok(Inbound::SwitchAnswer { .. }) => {}
                    Err(_) => break,
                }
            }
            let catalogue = announced_options(created.config_options, replay.updates());
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            Some(inbound) = updates.recv() => {
                // A `SwitchAnswer` here is impossible, same reasoning as
                // above.
                if let Inbound::Update(payload) = inbound {
                    replay.observe(payload);
                }
            }
```

with:

```rust
            Some(inbound) = updates.recv() => replay.inbound(inbound),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                while let Ok(inbound) = updates.try_recv() {
                    if let Inbound::Update(payload) = inbound {
                        replay.observe(payload);
                    }
                }
                let loaded = result.map_err(|err| StartError::acp(err, true))?;
                let catalogue = announced_options(loaded.config_options, &replay.kept);
```

with:

```rust
                while let Ok(inbound) = updates.try_recv() {
                    replay.inbound(inbound);
                }
                let loaded = result.map_err(|err| StartError::acp(err, true))?;
                let catalogue = announced_options(loaded.config_options, replay.updates());
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
fn announced_options(answered: Option<Vec<SessionConfigOption>>, before: &[Value]) -> Announced {
    match answered {
        Some(options) if !options.is_empty() => Announced {
            options,
            authoritative: true,
        },
        _ => Announced {
            options: before.iter().rev().find_map(config_update).unwrap_or_default(),
            authoritative: false,
        },
    }
}
```

with:

```rust
fn announced_options<'a>(
    answered: Option<Vec<SessionConfigOption>>,
    before: impl DoubleEndedIterator<Item = &'a Value>,
) -> Announced {
    match answered {
        Some(options) if !options.is_empty() => Announced {
            options,
            authoritative: true,
        },
        _ => Announced {
            options: before.rev().find_map(config_update).unwrap_or_default(),
            authoritative: false,
        },
    }
}

/// What the host tells the adapter it can do (ACP core §2.5, §6): boolean
/// config options, so agents offer them as booleans rather than on/off
/// selects, and form elicitation as `{"form": {}}`, never a boolean (P-19).
fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .session(
            ClientSessionCapabilities::new().config_options(
                SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new()),
            ),
        )
        .elicitation(ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()))
}

/// What a start that ran out of time was holding back (decision 2): the
/// questions the agent asked before the session existed, which nobody could
/// see or answer. Empty if there were none.
fn held_questions(replay: &Replay, updates: &mut mpsc::UnboundedReceiver<Inbound>) -> String {
    let mut kinds: Vec<PendingKind> = replay
        .kept
        .iter()
        .filter_map(|early| match early {
            Early::Question(question) => Some(question.kind),
            Early::Update(_) => None,
        })
        .collect();
    while let Ok(inbound) = updates.try_recv() {
        if let Inbound::Question(question) = inbound {
            kinds.push(question.kind);
        }
    }
    if kinds.is_empty() {
        return String::new();
    }
    let mut names: Vec<&str> = Vec::new();
    for kind in &kinds {
        let name = match kind {
            PendingKind::Permission => "permission",
            PendingKind::Elicitation => "elicitation",
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    format!(
        "; the agent asked {} question(s) during start-up ({}) that hennery cannot show before the session exists",
        kinds.len(),
        names.join("/")
    )
}
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        // Answers reach the session actor once it keeps its pending requests.
        CollectorFrame::AnswerPermission { request_id, .. } | CollectorFrame::AnswerElicitation { request_id, .. } => {
            uplink.reply(HostFrame::Error {
                request_id,
                code: "unsupported".into(),
                message: "this host does not take answers yet".into(),
            })
        }
```

with:

```rust
        CollectorFrame::AnswerPermission {
            request_id,
            session_id,
            pending_id,
            option_id,
        } => answer(
            uplink,
            sessions,
            request_id,
            &session_id,
            pending_id,
            Answer::Permission { option_id },
        ),
        CollectorFrame::AnswerElicitation {
            request_id,
            session_id,
            pending_id,
            action,
            content,
        } => answer(
            uplink,
            sessions,
            request_id,
            &session_id,
            pending_id,
            Answer::Elicitation { action, content },
        ),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
async fn send_pending<S>(sink: &mut S, uplink: &Uplink, sent: &mut HashMap<String, u64>) -> Result<()>
```

with:

```rust
/// An operator's answer goes to the session's live actor, which reports
/// `answer_result`. With no live actor there is no question to answer: the
/// rejection is correlated, like a prompt's, and the collector records the
/// answer as not delivered.
fn answer(
    uplink: &Uplink,
    sessions: &Sessions,
    request_id: String,
    session_id: &str,
    pending_id: String,
    answer: Answer,
) {
    match live_session(sessions, session_id) {
        Some(handle)
            if handle.send(SessionCmd::Answer {
                request_id: request_id.clone(),
                pending_id,
                answer,
            }) => {}
        _ => not_attached(uplink, request_id),
    }
}

async fn send_pending<S>(sink: &mut S, uplink: &Uplink, sent: &mut HashMap<String, u64>) -> Result<()>
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
use crate::session::{self, AgentCommand, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
```

with:

```rust
use crate::session::{self, AgentCommand, Answer, Attach, Launch, SessionCmd, SessionHandle, SessionOptions};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
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
```

with:

```rust
        // What the adapter sends before `session_started`, emitted after it.
        // Outside the start's future, so a start that runs out of time can
        // still say which questions it was holding back.
        let mut replay = Replay::default();
        let started = tokio::select! {
            result = async {
                let (session, catalogue) =
                    match tokio::time::timeout_at(deadline, negotiate(&conn, cwd, &attach, &mut updates, &mut replay)).await {
                        Ok(result) => result?,
                        Err(_) => {
                            return Err(StartError::other(format!(
                                "adapter did not start within {}s{}",
                                self.options.start_timeout.as_secs(),
                                held_questions(&replay, &mut updates)
                            )));
                        }
                    };
                let applied = apply_config(&conn, &session, catalogue, &config, self.options.config_timeout, deadline).await;
                Ok((session, applied))
            } => result,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// Returns the config options the adapter announced (none if it announced
/// none, or they did not parse).
async fn negotiate(
    conn: &ConnectionTo<Agent>,
    cwd: PathBuf,
    attach: &Attach,
    updates: &mut mpsc::UnboundedReceiver<Inbound>,
) -> Result<(SessionId, Replay, Announced), StartError> {
```

with:

```rust
/// Returns the config options the adapter announced (none if it announced
/// none, or they did not parse). What the adapter sends meanwhile goes into
/// `replay`.
async fn negotiate(
    conn: &ConnectionTo<Agent>,
    cwd: PathBuf,
    attach: &Attach,
    updates: &mut mpsc::UnboundedReceiver<Inbound>,
    replay: &mut Replay,
) -> Result<(SessionId, Announced), StartError> {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            return Ok((created.session_id, replay, catalogue));
```

with:

```rust
            return Ok((created.session_id, catalogue));
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    tokio::pin!(load);
    let mut replay = Replay::default();
    loop {
```

with:

```rust
    tokio::pin!(load);
    loop {
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                return Ok((id, replay, catalogue));
```

with:

```rust
                return Ok((id, catalogue));
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked`
Expected: all 280 tests pass. That includes eight new `host_session` tests, `answers_reach_the_actor_and_one_for_a_detached_session_is_not_attached` and the unit tests `form_elicitation_is_advertised_as_an_object` and `option_ids_are_read_from_the_raw_request`.

Check that `a_request_the_host_does_not_serve_is_refused_method_not_found` is a real guard. Replace the `None =>` arm of the request handler with `None => Ok(())`, which drops the responder. The test fails: it times out with the outbox stuck at `["session_started", "turn_started"]`. Restore the arm.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(host): forward the adapter's questions and deliver the operator's answers"
git push
```

### Task 4: The host closes open questions on stop, park, close and exit

**Files:**
- Modify: `crates/hennery-host/src/session.rs`
- Test: `crates/hennery-testkit/tests/host_session.rs`

**Interfaces:**
- Consumes (Task 3): `Questions`, `OpenQuestion`, `Actor::open_question`, `nth_pending`, `choose`, `verdicts`, `starting`, `asking` (test helpers).
- Produces: `Questions::cancelled_turn` and `fn cancelled_response(PendingKind) -> Value`. A permission gets `{"outcome": {"outcome": "cancelled"}}`, an elicitation `{"action": "cancel"}`.
- Produces: `Actor::cancel_questions(reason)`. It answers every open question cancelled and emits `pending_resolved{cancelled, reason}` for each, oldest first. `Actor::resolve_cancelled` does one question, and `Actor::has_questions` reports whether any is open.
- Produces: the hooks (decisions 5 and 6):
  - the first `Cancel` of a running turn: `turn_cancelled`;
  - a question the cancelled turn asks later: `turn_cancelled`, as it opens;
  - `teardown(…, reason)`: park and idle reap give `session_parked`, close gives `session_closed`, each after the turn's `turn_ended`;
  - the unanswered-cancel stop: `turn_cancelled`;
  - adapter exit: `adapter_lost`, after `turn_ended` and before `adapter_exited`;
  - the idle reaper's arm also requires `!self.has_questions()`.
- Produces: `Inbound::QuestionWithdrawn { pending_id }`, `struct Watcher` (aborted on drop), `OpenQuestion::_withdrawal`, `Actor::inbound: OnceLock<UnboundedSender<Inbound>>` and `Actor::withdraw_question` (decision 15).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/host_session.rs`:

```rust
// Plan (2): the host cancels open questions (ACP core §2.3, §4.6, §4.8).

/// The kinds from the first `pending_opened` on.
fn from_the_question(frames: &[HostFrame]) -> Vec<String> {
    let kinds = kinds(frames);
    let at = kinds
        .iter()
        .position(|k| k.starts_with("pending_opened"))
        .expect("a question");
    kinds[at..].to_vec()
}

#[tokio::test]
async fn a_cancel_answers_the_open_questions_cancelled_and_the_turn_ends_cancelled() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let handle = starting(&uplink, &asking(vec![FakeAsk::Permission]));
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let pending = nth_pending(&uplink, 0).await;
    assert!(handle.send(cancel("rc", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(
        from_the_question(&frames),
        [
            "pending_opened:permission",
            "pending_resolved:cancelled:turn_cancelled",
            "update:permission:cancelled",
            "turn_ended"
        ]
    );
    assert_eq!(turn_ends(&frames)[0].1, TurnOutcome::Cancelled);
    // The question is gone: a late answer reaches nobody.
    assert!(handle.send(choose("ra", &pending, "allow")));
    let frames = wait_until(&uplink, |f| !verdicts(f).is_empty()).await;
    assert_eq!(verdicts(&frames), [("ra".to_string(), pending, false)]);
}

#[tokio::test]
async fn a_question_asked_after_the_cancel_is_cancelled_at_once() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    // An adapter that keeps going after `session/cancel`: its second
    // question comes in after the cancel was sent.
    let script = FakeScript {
        ignore_cancel: true,
        chunks: vec![],
        ..asking(vec![FakeAsk::Permission, FakeAsk::Permission])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    nth_pending(&uplink, 0).await;
    assert!(handle.send(cancel("rc", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(
        from_the_question(&frames),
        [
            "pending_opened:permission",
            "pending_resolved:cancelled:turn_cancelled",
            "pending_opened:permission",
            "pending_resolved:cancelled:turn_cancelled",
            "update:permission:cancelled",
            "update:permission:cancelled",
            "turn_ended"
        ]
    );
}

#[tokio::test]
async fn park_and_close_cancel_the_open_questions_before_the_session_detaches() {
    for (cmd, last) in [
        (
            SessionCmd::Park {
                request_id: "rp".into(),
            },
            "session_parked:operator",
        ),
        (
            SessionCmd::Close {
                request_id: "rp".into(),
            },
            "session_closed",
        ),
    ] {
        let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
        let handle = starting(&uplink, &asking(vec![FakeAsk::Permission]));
        wait_until(&uplink, has("session_started")).await;
        assert!(handle.send(prompt("r1", "t1")));
        nth_pending(&uplink, 0).await;
        let reason = if last == "session_closed" {
            "session_closed"
        } else {
            "session_parked"
        };
        assert!(handle.send(cmd));
        let frames = wait_until(&uplink, has(last)).await;
        assert_eq!(
            from_the_question(&frames),
            [
                "pending_opened:permission".to_string(),
                "turn_ended".to_string(),
                format!("pending_resolved:cancelled:{reason}"),
                last.to_string()
            ]
        );
    }
}

#[tokio::test]
async fn an_adapter_lost_with_a_question_open_cancels_it_adapter_lost() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        crash_while_asking: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("session_parked:adapter_exited")).await;
    // The exit watcher's order (ACP core §2.3).
    assert_eq!(
        from_the_question(&frames),
        [
            "pending_opened:permission",
            "turn_ended",
            "pending_resolved:cancelled:adapter_lost",
            "adapter_exited",
            "session_parked:adapter_exited"
        ]
    );
    wait_ended(&handle).await;
}

/// No timeout on a question (ACP core §4.6, scenario 10): one asked
/// outside any turn keeps the session through many idle windows, and is
/// still answered.
#[tokio::test]
async fn the_reaper_never_parks_a_session_with_a_question_open() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        ask_on_load: true,
        ..asking(vec![FakeAsk::Permission])
    };
    let handle = session::resume(
        uplink.clone(),
        "r0".into(),
        "s1".into(),
        "agent-7".into(),
        fake_with(&script),
        std::env::temp_dir(),
        SessionOptions {
            idle_timeout: Some(Duration::from_millis(100)),
            ..SessionOptions::default()
        },
    );
    let pending = nth_pending(&uplink, 0).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        kinds(&uplink.pending().unwrap()),
        ["session_started", "pending_opened:permission"],
        "ten idle windows passed with the question open"
    );
    assert!(handle.send(choose("ra", &pending, "allow")));
    wait_until(&uplink, has("update:permission:selected:allow")).await;
    // Answered, the session is idle again: now the reaper parks it.
    wait_until(&uplink, has("session_parked:idle")).await;
}

/// The adapter may withdraw its own question (`$/cancel_request`): the
/// question closes, the adapter hears the cancellation error, and an answer
/// given afterwards reaches nobody.
#[tokio::test]
async fn a_question_the_agent_withdraws_closes_and_an_answer_reaches_nobody() {
    let (uplink, _replies) = Uplink::new(Outbox::open_in_memory().unwrap());
    let script = FakeScript {
        withdraw_asks: true,
        chunks: vec![],
        ..asking(vec![FakeAsk::Permission])
    };
    let handle = starting(&uplink, &script);
    wait_until(&uplink, has("session_started")).await;
    assert!(handle.send(prompt("r1", "t1")));
    let frames = wait_until(&uplink, has("turn_ended")).await;
    assert_eq!(
        from_the_question(&frames),
        [
            "pending_opened:permission",
            "pending_resolved:cancelled:agent_withdrew",
            "update:permission:error:-32800",
            "turn_ended"
        ]
    );
    let pending = opened(&frames)[0].0.id.clone();
    assert!(handle.send(choose("ra", &pending, "allow")));
    let frames = wait_until(&uplink, |f| !verdicts(f).is_empty()).await;
    assert_eq!(verdicts(&frames), [("ra".to_string(), pending, false)]);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-testkit --test host_session --locked`
Expected: 6 FAIL (68 pass):
- `a_cancel_…` and `a_question_asked_after_the_cancel_…` time out waiting for `turn_ended`: nothing answers the question, so the fake waits.
- `park_and_close_…` is missing `pending_resolved:cancelled:session_parked`.
- `an_adapter_lost_…` is missing `pending_resolved:cancelled:adapter_lost`.
- `the_reaper_never_parks_…` finds `session_parked:idle` after the question.
- `a_question_the_agent_withdraws_…` times out waiting for `turn_ended`: the withdrawal is ignored, and the fake waits for the answer to the request it withdrew.

- [ ] **Step 3: Cancel the open questions**

In `crates/hennery-host/src/session.rs`, replace:

```rust
/// The adapter's questions waiting for the operator (ACP core §4.6), oldest
/// first. No timeout: each waits until it is answered or cancelled.
#[derive(Default)]
struct Questions {
    open: Vec<OpenQuestion>,
}
```

with:

```rust
/// The adapter's questions waiting for the operator (ACP core §4.6), oldest
/// first. No timeout: each waits until it is answered or cancelled.
#[derive(Default)]
struct Questions {
    open: Vec<OpenQuestion>,
    /// The turn a `session/cancel` went out for: a question it asks from
    /// then on is cancelled as soon as it opens.
    cancelled_turn: Option<String>,
}

/// The ACP answer that tells the adapter a question is off: a permission's
/// `cancelled` outcome, an elicitation's `cancel` action.
fn cancelled_response(kind: PendingKind) -> Value {
    match kind {
        PendingKind::Permission => serde_json::json!({"outcome": {"outcome": "cancelled"}}),
        PendingKind::Elicitation => serde_json::json!({"action": "cancel"}),
    }
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                            if running.cancel_deadline.is_none() {
                                if let Err(err) = conn.send_notification(CancelNotification::new(agent_session.clone())) {
                                    tracing::warn!(session_id = %self.session_id, error = %err, "session/cancel not sent");
                                }
                                running.cancel_deadline = Some(Instant::now() + self.options.cancel_grace);
                            }
```

with:

```rust
                            if running.cancel_deadline.is_none() {
                                if let Err(err) = conn.send_notification(CancelNotification::new(agent_session.clone())) {
                                    tracing::warn!(session_id = %self.session_id, error = %err, "session/cancel not sent");
                                }
                                running.cancel_deadline = Some(Instant::now() + self.options.cancel_grace);
                                // ACP: after `session/cancel`, every pending
                                // request is answered cancelled.
                                self.questions.lock().expect("questions lock").cancelled_turn = Some(turn_id);
                                self.cancel_questions(PendingReason::TurnCancelled);
                            }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    Some(SessionCmd::Park { .. }) => {
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs).await;
                        return self.emit(SessionBody::SessionParked { reason: ParkReason::Operator });
                    }
                    Some(SessionCmd::Close { .. }) => {
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs).await;
                        return self.emit(SessionBody::SessionClosed);
                    }
```

with:

```rust
                    Some(SessionCmd::Park { .. }) => {
                        let reason = PendingReason::SessionParked;
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs, reason).await;
                        return self.emit(SessionBody::SessionParked { reason: ParkReason::Operator });
                    }
                    Some(SessionCmd::Close { .. }) => {
                        let reason = PendingReason::SessionClosed;
                        self.teardown(&mut adapter, &mut updates, turn.take(), &mut configs, reason).await;
                        return self.emit(SessionBody::SessionClosed);
                    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                _ = idle_deadline(self.options.idle_timeout, idle_since),
                    if turn.is_none() && configs.out.is_none() && configs.orphan.is_none() =>
                {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    self.begin_ending();
                    self.teardown(&mut adapter, &mut updates, None, &mut configs).await;
```

with:

```rust
                // Never with a question open, in a turn or not: it has no
                // timeout (ACP core §4.6).
                _ = idle_deadline(self.options.idle_timeout, idle_since),
                    if turn.is_none() && configs.out.is_none() && configs.orphan.is_none() && !self.has_questions() =>
                {
                    tracing::info!(session_id = %self.session_id, "reaping idle session");
                    self.begin_ending();
                    self.teardown(&mut adapter, &mut updates, None, &mut configs, PendingReason::SessionParked).await;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        self.emit(SessionBody::PendingOpened {
            pending_id: pending_id.clone(),
            indexed: Indexed {
                turn_id: turn.map(str::to_string),
                pending: Some(PendingExtract {
                    id: pending_id.clone(),
                    kind,
                    option_ids,
                }),
                ..Indexed::default()
            },
            payload: params,
        });
        self.questions.lock().expect("questions lock").open.push(OpenQuestion {
            pending_id,
            kind,
            responder,
        });
    }
```

with:

```rust
        self.emit(SessionBody::PendingOpened {
            pending_id: pending_id.clone(),
            indexed: Indexed {
                turn_id: turn.map(str::to_string),
                pending: Some(PendingExtract {
                    id: pending_id.clone(),
                    kind,
                    option_ids,
                }),
                ..Indexed::default()
            },
            payload: params,
        });
        let question = OpenQuestion {
            pending_id,
            kind,
            responder,
        };
        let mut questions = self.questions.lock().expect("questions lock");
        if turn.is_some() && questions.cancelled_turn.as_deref() == turn {
            // Asked in a turn the operator already stopped.
            drop(questions);
            self.resolve_cancelled(question, PendingReason::TurnCancelled);
        } else {
            questions.open.push(question);
        }
    }

    fn has_questions(&self) -> bool {
        !self.questions.lock().expect("questions lock").open.is_empty()
    }

    /// Tell the adapter every open question is off, and the collector why
    /// (ACP core §4.6): `pending_resolved{cancelled, reason}` each, oldest
    /// first.
    fn cancel_questions(&self, reason: PendingReason) {
        let open = std::mem::take(&mut self.questions.lock().expect("questions lock").open);
        for question in open {
            self.resolve_cancelled(question, reason);
        }
    }

    fn resolve_cancelled(&self, question: OpenQuestion, reason: PendingReason) {
        // An adapter that is gone cannot hear it; the collector still must.
        if let Err(err) = question.responder.respond(cancelled_response(question.kind)) {
            tracing::debug!(session_id = %self.session_id, error = %err, "cancellation not sent to the adapter");
        }
        self.emit(SessionBody::PendingResolved {
            pending_id: question.pending_id,
            resolution: PendingResolution::Cancelled,
            reason: Some(reason),
        });
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// Park or close: forward any output already queued, end the turn as
    /// interrupted, then kill the group.
    async fn teardown(
        &self,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Inbound>,
        turn: Option<Turn>,
        configs: &mut PendingConfigs,
    ) {
        self.drain_updates(updates, turn.as_ref().map(|t| t.id.as_str()), configs);
        if let Some(turn) = turn {
            self.end_turn(turn.id, TurnOutcome::Interrupted, None, None);
        }
        adapter.terminate(self.options.kill_grace).await;
    }
```

with:

```rust
    /// Park or close: forward any output already queued, end the turn as
    /// interrupted, cancel the open questions for `reason` (ACP core §4.8),
    /// then kill the group.
    async fn teardown(
        &self,
        adapter: &mut Adapter,
        updates: &mut mpsc::UnboundedReceiver<Inbound>,
        turn: Option<Turn>,
        configs: &mut PendingConfigs,
        reason: PendingReason,
    ) {
        self.drain_updates(updates, turn.as_ref().map(|t| t.id.as_str()), configs);
        if let Some(turn) = turn {
            self.end_turn(turn.id, TurnOutcome::Interrupted, None, None);
        }
        self.cancel_questions(reason);
        adapter.terminate(self.options.kill_grace).await;
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let message = format!("the adapter did not stop within {grace:?} of session/cancel");
        self.end_turn(turn.id, TurnOutcome::Cancelled, None, Some(message.clone()));
        adapter.terminate(self.options.kill_grace).await;
```

with:

```rust
        let message = format!("the adapter did not stop within {grace:?} of session/cancel");
        self.end_turn(turn.id, TurnOutcome::Cancelled, None, Some(message.clone()));
        // The cancel already answered the turn's questions; any asked
        // outside it go the same way.
        self.cancel_questions(PendingReason::TurnCancelled);
        adapter.terminate(self.options.kill_grace).await;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The exit watcher's steps (ACP core §2.3): outstanding calls fail (the
    /// reply future is dropped), the turn ends `interrupted`, then
    /// `adapter_exited` and `session_parked{adapter_exited}`.
```

with:

```rust
    /// The exit watcher's steps (ACP core §2.3): outstanding calls fail (the
    /// reply future is dropped), the turn ends `interrupted`, the open
    /// questions are cancelled `adapter_lost`, then `adapter_exited` and
    /// `session_parked{adapter_exited}`.
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                TurnOutcome::Interrupted,
                None,
                Some("the adapter exited".into()),
            );
        }
        let stderr_tail = adapter.stderr_tail().await;
```

with:

```rust
                TurnOutcome::Interrupted,
                None,
                Some("the adapter exited".into()),
            );
        }
        self.cancel_questions(PendingReason::AdapterLost);
        let stderr_tail = adapter.stderr_tail().await;
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
use std::sync::{Arc, Mutex};
```

with:

```rust
use std::sync::{Arc, Mutex, OnceLock};
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// A question for the operator, in wire order with the notifications
    /// around it: it follows the tool call it asks about.
    Question(Box<Question>),
}
```

with:

```rust
    /// A question for the operator, in wire order with the notifications
    /// around it: it follows the tool call it asks about.
    Question(Box<Question>),
    /// The adapter withdrew question `pending_id` (`$/cancel_request`).
    QuestionWithdrawn {
        pending_id: String,
    },
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
struct OpenQuestion {
    pending_id: String,
    kind: PendingKind,
    responder: Responder<Value>,
}
```

with:

```rust
struct OpenQuestion {
    pending_id: String,
    kind: PendingKind,
    responder: Responder<Value>,
    /// Watches for the adapter withdrawing the question; stops with it.
    _withdrawal: Option<Watcher>,
}

/// A task that is aborted when its owner is dropped.
struct Watcher(tokio::task::JoinHandle<()>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            // No switch is sent before the actor's main loop starts.
            Inbound::SwitchAnswer { .. } => {}
```

with:

```rust
            // No switch is sent, and no question is open, before the
            // actor's main loop starts.
            Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. } => {}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    /// The adapter's questions waiting for the operator.
    questions: Mutex<Questions>,
```

with:

```rust
    /// The adapter's questions waiting for the operator.
    questions: Mutex<Questions>,
    /// The inbound channel, for the questions' withdrawal watchers.
    inbound: OnceLock<mpsc::UnboundedSender<Inbound>>,
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        questions: Mutex::new(Questions::default()),
```

with:

```rust
        questions: Mutex::new(Questions::default()),
        inbound: OnceLock::new(),
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let switch_tx = updates_tx.clone();
        let questions_tx = updates_tx.clone();
```

with:

```rust
        let switch_tx = updates_tx.clone();
        let questions_tx = updates_tx.clone();
        let _ = self.inbound.set(updates_tx.clone());
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                Ok(Inbound::Question(question)) => early.push(Early::Question(question)),
                Ok(Inbound::SwitchAnswer { .. }) => {}
```

with:

```rust
                Ok(Inbound::Question(question)) => early.push(Early::Question(question)),
                Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. }) => {}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
                    Ok(Inbound::Question(question)) => replay.kept.push(Early::Question(question)),
                    Ok(Inbound::SwitchAnswer { .. }) => {}
```

with:

```rust
                    Ok(Inbound::Question(question)) => replay.kept.push(Early::Question(question)),
                    Ok(Inbound::SwitchAnswer { .. } | Inbound::QuestionWithdrawn { .. }) => {}
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
            Inbound::Question(question) => {
                self.open_question(question, turn);
                false
            }
        }
    }
```

with:

```rust
            Inbound::Question(question) => {
                self.open_question(question, turn);
                false
            }
            // The session may be idle now: the reaper's clock restarts.
            Inbound::QuestionWithdrawn { pending_id } => {
                self.withdraw_question(pending_id);
                true
            }
        }
    }
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
        let question = OpenQuestion {
            pending_id,
            kind,
            responder,
        };
```

with:

```rust
        // The adapter may withdraw its question (`$/cancel_request`): a
        // watcher reports that through the ordered channel, and stops when
        // the question is answered or cancelled.
        let cancellation = responder.cancellation();
        let withdrawal = self.inbound.get().cloned().map(|inbound| {
            let pending_id = pending_id.clone();
            Watcher(tokio::spawn(async move {
                cancellation.cancelled().await;
                let _ = inbound.send(Inbound::QuestionWithdrawn { pending_id });
            }))
        });
        let question = OpenQuestion {
            pending_id,
            kind,
            responder,
            _withdrawal: withdrawal,
        };
```

In `crates/hennery-host/src/session.rs`, replace:

```rust
    fn has_questions(&self) -> bool {
```

with:

```rust
    /// The adapter withdrew a question (`$/cancel_request`): nobody waits
    /// for its answer any more, so the operator can no longer give one
    /// (`pending_resolved{cancelled, agent_withdrew}`). The request is
    /// answered with the standard cancellation error, as JSON-RPC expects.
    fn withdraw_question(&self, pending_id: String) {
        let question = {
            let mut questions = self.questions.lock().expect("questions lock");
            let at = questions.open.iter().position(|q| q.pending_id == pending_id);
            at.map(|at| questions.open.remove(at))
        };
        // Already answered or cancelled: nothing is left to withdraw.
        let Some(question) = question else {
            return;
        };
        let cancelled = agent_client_protocol::Error::request_cancelled();
        if let Err(err) = question.responder.respond_with_error(cancelled) {
            tracing::debug!(session_id = %self.session_id, error = %err, "withdrawal not acknowledged to the adapter");
        }
        self.emit(SessionBody::PendingResolved {
            pending_id,
            resolution: PendingResolution::Cancelled,
            reason: Some(PendingReason::AgentWithdrew),
        });
    }

    fn has_questions(&self) -> bool {
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked`
Expected: all 286 tests pass. `the_reaper_never_parks_a_session_with_a_question_open` is scenario 10 on the host: ten idle windows with a question open, then the answer is still delivered. The only thing it measures against the clock is the absence of a park.

Check that `a_question_the_agent_withdraws_closes_and_an_answer_reaches_nobody` is a real guard. In the watcher, replace `let _ = inbound.send(Inbound::QuestionWithdrawn { pending_id });` with `let _ = (inbound, pending_id);`: the test times out with the outbox at `[…, "pending_opened:permission"]`. Restore it.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(host): cancel open questions on stop, park, close and adapter exit"
git push
```

### Task 5: The collector's pending set and answer queue

**Files:**
- Modify: `crates/hennery-sessions/src/store.rs`
- Test: `crates/hennery-sessions/tests/store.rs`

**Interfaces:**
- Consumes (Task 1): the three bodies, `PendingExtract`, `PendingState`, `PendingItem`, `AnswerRequest`, `CollectorFrame::{AnswerPermission, AnswerElicitation}`.
- Produces: migration 6, with `pending(pending_id PK, session_id, kind, turn_id, option_ids, payload, state, reason, opened_at, resolved_at)` and `answer_queue(pending_id PK, session_id, request_id UNIQUE, answer, submitted_at, delivered NULL)`.
- Produces, on ingest:
  - `pending_opened` inserts an open question and turns `running` into `blocked`. It applies only to an attached session and a turn still open (decision 14).
  - `pending_resolved` moves an open question to `delivered` / `cancelled` (`resolve_pending`), which returns `blocked` to `running` once none is open.
  - `answer_result` folds into `answer_queue.delivered`, and `true` sticks.
  - A cancelled question's queued answer gets `delivered = 0`.
- Produces, collector-side, `pending_cancelled{pending_id, reason}` events (`cancel_open_pending`, decision 9). They come from:
  - `reconcile_host`, when an attached session is missing (`host_restarted`);
  - `close_in` (`session_closed`);
  - a host `session_parked` that applies (`adapter_lost` or `session_parked`), or a `session_closed` that applies (`session_closed`).
- Produces, the public API:
  - `pub enum AnswerSubmission { Queued(Box<QueuedAnswer>), NotFound, NotOpen, AlreadyAnswered, Invalid(String) }`
  - `pub struct QueuedAnswer { event: EventDto, host_id: String, request_id: String, frame: CollectorFrame }`
  - `Store::submit_answer(&self, session_id: &str, pending_id: &str, answer: &AnswerRequest) -> Result<AnswerSubmission>`
  - `Store::answers_to_send(&self, host_id: &str) -> Result<Vec<CollectorFrame>>`
  - `Store::open_pending(&self, session_id: &str) -> Result<Vec<PendingItem>>`
  - `Store::pending_item(&self, pending_id: &str) -> Result<Option<PendingItem>>`

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-sessions/tests/store.rs`:

```rust
// Plan (2): the pending set and the answer queue (ACP core §4.6, §5, §8).

use hennery_proto::frames::{
    CollectorFrame, ElicitationAction, PendingExtract, PendingKind, PendingReason, PendingResolution,
};
use hennery_proto::rest::{AnswerRequest, PendingState};
use hennery_sessions::store::AnswerSubmission;

/// `s1` active with turn `t1` running (seqs 1 and 2).
fn running(store: &Store) {
    started(store);
    store.open_turn("s1", "t1", &prompt_text()).unwrap();
    store.ingest("s1", 2, &turn_started("t1")).unwrap();
}

fn permission(pending_id: &str) -> SessionBody {
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
            }),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    }
}

fn elicitation(pending_id: &str) -> SessionBody {
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: Some("t1".into()),
            pending: Some(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Elicitation,
                option_ids: None,
            }),
            ..Indexed::default()
        },
        payload: json!({"mode": "form"}),
    }
}

fn resolved(pending_id: &str, reason: Option<PendingReason>) -> SessionBody {
    SessionBody::PendingResolved {
        pending_id: pending_id.into(),
        resolution: if reason.is_some() {
            PendingResolution::Cancelled
        } else {
            PendingResolution::Delivered
        },
        reason,
    }
}

fn verdict(pending_id: &str, delivered: bool) -> SessionBody {
    SessionBody::AnswerResult {
        pending_id: pending_id.into(),
        request_id: "whatever".into(),
        delivered,
    }
}

fn choose(option_id: &str) -> AnswerRequest {
    AnswerRequest::Permission {
        option_id: option_id.into(),
    }
}

fn activity(store: &Store) -> Option<String> {
    store.session("s1").unwrap().unwrap().activity
}

fn state_of(store: &Store, pending_id: &str) -> (PendingState, Option<PendingReason>) {
    let item = store.pending_item(pending_id).unwrap().unwrap();
    (item.state, item.reason)
}

#[test]
fn a_running_turn_is_blocked_until_its_last_open_question_is_resolved() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.ingest("s1", 4, &elicitation("p2")).unwrap();
    assert_eq!(activity(&store).as_deref(), Some("blocked"));
    let open: Vec<String> = store
        .open_pending("s1")
        .unwrap()
        .into_iter()
        .map(|p| p.pending_id)
        .collect();
    assert_eq!(open, ["p1", "p2"], "oldest first");
    store.ingest("s1", 5, &resolved("p1", None)).unwrap();
    assert_eq!(activity(&store).as_deref(), Some("blocked"), "p2 is still open");
    store
        .ingest("s1", 6, &resolved("p2", Some(PendingReason::TurnCancelled)))
        .unwrap();
    assert_eq!(activity(&store).as_deref(), Some("running"));
    assert_eq!(
        state_of(&store, "p2"),
        (PendingState::Cancelled, Some(PendingReason::TurnCancelled))
    );
    assert!(store.open_pending("s1").unwrap().is_empty());
    // A second resolution of the same request is stored, not applied.
    assert!(store.ingest("s1", 7, &resolved("p2", None)).unwrap().is_empty());
    assert_eq!(state_of(&store, "p2").0, PendingState::Cancelled);
}

#[test]
fn a_question_for_a_detached_session_or_an_ended_turn_is_not_applied() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &ended("t1")).unwrap();
    assert!(store.ingest("s1", 4, &permission("p1")).unwrap().is_empty());
    assert!(store.pending_item("p1").unwrap().is_none());
    parked(&store, 5);
    assert!(store.ingest("s1", 6, &permission("p2")).unwrap().is_empty());
}

#[test]
fn an_answer_is_queued_once_and_only_if_it_fits_the_question() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.ingest("s1", 4, &elicitation("p2")).unwrap();
    let invalid = |answer: AnswerRequest, pending: &str| {
        matches!(
            store.submit_answer("s1", pending, &answer).unwrap(),
            AnswerSubmission::Invalid(_)
        )
    };
    assert!(invalid(choose("maybe"), "p1"), "an option it does not offer");
    let decline_with_content = AnswerRequest::Elicitation {
        action: ElicitationAction::Decline,
        content: Some(json!({"name": "x"})),
    };
    assert!(invalid(decline_with_content, "p2"));
    assert!(invalid(choose("allow"), "p2"), "an option for an elicitation");
    let AnswerSubmission::Queued(queued) = store.submit_answer("s1", "p1", &choose("allow")).unwrap() else {
        panic!("not queued");
    };
    assert_eq!(
        (queued.event.kind.as_str(), queued.host_id.as_str()),
        ("answer_submitted", "h1")
    );
    assert_eq!(queued.event.body["pending_id"], "p1");
    assert_eq!(
        queued.frame,
        CollectorFrame::AnswerPermission {
            request_id: queued.request_id.clone(),
            session_id: "s1".into(),
            pending_id: "p1".into(),
            option_id: "allow".into(),
        }
    );
    assert_eq!(
        store.submit_answer("s1", "p1", &choose("reject")).unwrap(),
        AnswerSubmission::AlreadyAnswered
    );
    assert_eq!(
        store.submit_answer("s1", "nope", &choose("allow")).unwrap(),
        AnswerSubmission::NotFound
    );
    assert_eq!(
        store.submit_answer("other", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::NotFound,
        "a pending id is looked up within its own session"
    );
    store
        .ingest("s1", 5, &resolved("p2", Some(PendingReason::TurnCancelled)))
        .unwrap();
    let accept = AnswerRequest::Elicitation {
        action: ElicitationAction::Accept,
        content: Some(json!({"name": "x"})),
    };
    assert_eq!(
        store.submit_answer("s1", "p2", &accept).unwrap(),
        AnswerSubmission::NotOpen
    );
    let item = store.pending_item("p1").unwrap().unwrap();
    assert_eq!((item.answered, item.delivered), (true, None));
}

#[test]
fn a_delivered_verdict_sticks_and_a_later_false_does_not_overwrite_it() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.submit_answer("s1", "p1", &choose("allow")).unwrap();
    store.ingest("s1", 4, &verdict("p1", true)).unwrap();
    store.ingest("s1", 5, &resolved("p1", None)).unwrap();
    // A resent answer the host no longer had a waiter for.
    store.ingest("s1", 6, &verdict("p1", false)).unwrap();
    let item = store.pending_item("p1").unwrap().unwrap();
    assert_eq!((item.state, item.delivered), (PendingState::Delivered, Some(true)));
}

#[test]
fn only_answers_still_waiting_for_a_verdict_on_an_open_question_are_sent() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.ingest("s1", 4, &permission("p2")).unwrap();
    store.ingest("s1", 5, &permission("p3")).unwrap();
    for p in ["p1", "p2", "p3"] {
        let queued = store.submit_answer("s1", p, &choose("allow")).unwrap();
        assert!(matches!(queued, AnswerSubmission::Queued(_)), "{p} not queued");
    }
    let pending_ids = |frames: Vec<CollectorFrame>| -> Vec<String> {
        frames
            .into_iter()
            .map(|f| match f {
                CollectorFrame::AnswerPermission { pending_id, .. } => pending_id,
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(pending_ids(store.answers_to_send("h1").unwrap()), ["p1", "p2", "p3"]);
    assert!(store.answers_to_send("another-host").unwrap().is_empty());
    // p1 got its verdict; p2's question was cancelled before its answer
    // could be sent, so the answer can never be delivered.
    store.ingest("s1", 6, &verdict("p1", true)).unwrap();
    store
        .ingest("s1", 7, &resolved("p2", Some(PendingReason::TurnCancelled)))
        .unwrap();
    assert_eq!(pending_ids(store.answers_to_send("h1").unwrap()), ["p3"]);
    assert_eq!(store.pending_item("p2").unwrap().unwrap().delivered, Some(false));
    // p3's goes again after every handshake until its verdict comes.
    assert_eq!(pending_ids(store.answers_to_send("h1").unwrap()), ["p3"]);
}

#[test]
fn a_host_restart_cancels_open_questions_and_a_presumed_park_keeps_them() {
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.presume_parked("h1").unwrap();
    assert_eq!(
        state_of(&store, "p1").0,
        PendingState::Open,
        "the host may still hold it"
    );
    store.reconcile_host("h1", &[attached("s1", Some("t1"))]).unwrap();
    assert_eq!(state_of(&store, "p1").0, PendingState::Open, "reattached, intact");
    // Away again, and this time back without the session: a restart found
    // through a presumed park.
    store.presume_parked("h1").unwrap();
    let r = store.reconcile_host("h1", &[]).unwrap();
    assert_eq!(
        kinds(&r.events),
        ["host_restarted", "turn_ended_synthesized", "pending_cancelled"]
    );
    assert_eq!(
        r.events[2].body,
        json!({"pending_id": "p1", "reason": "host_restarted"})
    );
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::HostRestarted))
    );
}

#[test]
fn a_detach_or_an_unattached_close_cancels_whatever_is_still_open() {
    // The host resolves its questions before it detaches; a question it
    // left open is cancelled with the detach.
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    let created = store
        .ingest(
            "s1",
            4,
            &SessionBody::SessionParked {
                reason: ParkReason::AdapterExited,
            },
        )
        .unwrap();
    assert_eq!(kinds(&created).last(), Some(&"pending_cancelled"));
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::AdapterLost))
    );
    // Closing a session whose host is away closes its questions too.
    let store = Store::open_in_memory().unwrap();
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    store.presume_parked("h1").unwrap();
    let events = store.close_now("s1").unwrap();
    assert!(kinds(&events).contains(&"pending_cancelled"), "{:?}", kinds(&events));
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::SessionClosed))
    );
}
```

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
             DROP TABLE session_catalog;
             PRAGMA user_version = 1;",
```

with:

```rust
             DROP TABLE session_catalog;
             DROP TABLE answer_queue;
             DROP TABLE pending;
             PRAGMA user_version = 1;",
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-sessions --test store --locked`
Expected: FAIL to compile, with `error[E0432]: unresolved import hennery_sessions::store::AnswerSubmission` and `error[E0599]: no method named open_pending found for struct Store`.

- [ ] **Step 3: Keep the pending set and the queue**

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
use anyhow::Result;
use hennery_proto::frames::{AttachedSession, ConfigValue, Indexed, SessionBody, SessionConfig, TurnOutcome};
use hennery_proto::rest::{EventDto, SessionCatalog};
```

with:

```rust
use anyhow::Result;
use hennery_proto::frames::{
    AttachedSession, CollectorFrame, ConfigValue, ElicitationAction, Indexed, ParkReason, PendingKind, PendingReason,
    PendingResolution, SessionBody, SessionConfig, TurnOutcome,
};
use hennery_proto::rest::{AnswerRequest, EventDto, PendingItem, PendingState, SessionCatalog};
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    CREATE TABLE session_catalog (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        config_options TEXT NOT NULL,
        updated_at TEXT NOT NULL);
",
];
```

with:

```rust
    CREATE TABLE session_catalog (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        config_options TEXT NOT NULL,
        updated_at TEXT NOT NULL);
",
    // Permission and elicitation (ACP core §4.6, §8): the pending set, which
    // is canonical here, and the durable answer queue, keyed by pending_id.
    // `delivered` stays NULL until a verdict: `answer_result`, a host's
    // refusal, or the question's cancellation.
    "
    CREATE TABLE pending (
        pending_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        kind TEXT NOT NULL,
        turn_id TEXT,
        option_ids TEXT,
        payload TEXT NOT NULL,
        state TEXT NOT NULL,
        reason TEXT,
        opened_at TEXT NOT NULL,
        resolved_at TEXT);
    CREATE INDEX pending_by_session ON pending(session_id, state);
    CREATE TABLE answer_queue (
        pending_id TEXT PRIMARY KEY REFERENCES pending(pending_id),
        session_id TEXT NOT NULL REFERENCES sessions(id),
        request_id TEXT NOT NULL UNIQUE,
        answer TEXT NOT NULL,
        submitted_at TEXT NOT NULL,
        delivered INTEGER);
",
];
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
/// What the collector did after a host's `resend_complete` (ACP core §5.1).
```

with:

```rust
/// The outcome of `Store::submit_answer` (ACP core §4.6).
#[derive(Debug, PartialEq)]
pub enum AnswerSubmission {
    Queued(Box<QueuedAnswer>),
    /// No such pending request in that session.
    NotFound,
    /// Answered or cancelled already.
    NotOpen,
    /// An answer is queued for it already.
    AlreadyAnswered,
    /// The answer does not fit the question (why).
    Invalid(String),
}

/// An answer the collector has queued durably.
#[derive(Debug, PartialEq)]
pub struct QueuedAnswer {
    /// Its `answer_submitted` event.
    pub event: EventDto,
    /// Where `frame` goes: now if that host is connected and reconciled,
    /// else after its next handshake.
    pub host_id: String,
    pub request_id: String,
    pub frame: CollectorFrame,
}

/// What the collector did after a host's `resend_complete` (ACP core §5.1).
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
/// Keep a stored host fact that did not apply as the idempotency key only:
```

with:

```rust
/// A wire enum's snake_case name, as stored.
fn tag(value: impl serde::Serialize) -> Result<String> {
    Ok(serde_json::to_value(value)?.as_str().unwrap_or_default().to_string())
}

/// A stored snake_case name back as its wire enum.
fn untag<T: serde::de::DeserializeOwned>(name: String) -> Result<T> {
    Ok(serde_json::from_value(Value::String(name))?)
}

/// Move an open pending request of `session_id` to `state` (with `reason`
/// if cancelled). An answer queued for a cancelled one can never be
/// delivered any more, so it gets its verdict. A session blocked on
/// nothing else runs again. `false` if the request was not open.
fn resolve_pending(
    tx: &Transaction<'_>,
    session_id: &str,
    pending_id: &str,
    state: PendingState,
    reason: Option<PendingReason>,
    ts: &str,
) -> Result<bool> {
    let changed = tx.execute(
        "UPDATE pending SET state = ?3, reason = ?4, resolved_at = ?5
         WHERE pending_id = ?1 AND session_id = ?2 AND state = 'open'",
        params![pending_id, session_id, tag(state)?, reason.map(tag).transpose()?, ts],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    if state == PendingState::Cancelled {
        tx.execute(
            "UPDATE answer_queue SET delivered = 0 WHERE pending_id = ?1 AND delivered IS NULL",
            [pending_id],
        )?;
    }
    tx.execute(
        "UPDATE sessions SET activity = 'running'
         WHERE id = ?1 AND activity = 'blocked'
             AND NOT EXISTS (SELECT 1 FROM pending WHERE session_id = ?1 AND state = 'open')",
        [session_id],
    )?;
    Ok(true)
}

/// Cancel, collector-side, every pending request of `session_id` that is
/// still open, with one `pending_cancelled` event each (ACP core §4.6,
/// §5.2): the host will never resolve them (it restarted, or the session
/// is gone), and a question must not stay answerable.
fn cancel_open_pending(
    tx: &Transaction<'_>,
    session_id: &str,
    reason: PendingReason,
    ts: &str,
) -> Result<Vec<EventDto>> {
    let ids: Vec<String> = {
        let mut stmt =
            tx.prepare("SELECT pending_id FROM pending WHERE session_id = ?1 AND state = 'open' ORDER BY rowid")?;
        let rows = stmt.query_map([session_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut events = Vec::new();
    for id in ids {
        resolve_pending(tx, session_id, &id, PendingState::Cancelled, Some(reason), ts)?;
        let body = json!({ "pending_id": id, "reason": reason });
        events.push(collector_event(tx, session_id, "pending_cancelled", body, ts)?);
    }
    Ok(events)
}

/// Whether `answer` fits a pending request of `kind` (ACP core §4.6): one
/// of the stored options for a permission request, an action for an
/// elicitation, with content (an object) only to accept.
fn check_answer(kind: PendingKind, option_ids: Option<&[String]>, answer: &AnswerRequest) -> Result<(), String> {
    match (kind, answer) {
        (PendingKind::Permission, AnswerRequest::Permission { option_id }) => match option_ids {
            Some(ids) if ids.contains(option_id) => Ok(()),
            Some(_) => Err(format!("the request offers no option {option_id}")),
            None => Err("the request's options could not be read; stop, park or close the session".into()),
        },
        (PendingKind::Elicitation, AnswerRequest::Elicitation { action, content }) => match (action, content) {
            (_, None) => Ok(()),
            (ElicitationAction::Accept, Some(content)) if content.is_object() => Ok(()),
            (ElicitationAction::Accept, Some(_)) => Err("the form's content must be an object".into()),
            (_, Some(_)) => Err("only an accepted form has content".into()),
        },
        (PendingKind::Permission, _) => Err("a permission request is answered with an option_id".into()),
        (PendingKind::Elicitation, _) => Err("an elicitation is answered with an action".into()),
    }
}

/// The frame that carries a queued answer to its host.
fn answer_frame(request_id: String, session_id: &str, pending_id: &str, answer: AnswerRequest) -> CollectorFrame {
    let (session_id, pending_id) = (session_id.to_string(), pending_id.to_string());
    match answer {
        AnswerRequest::Permission { option_id } => CollectorFrame::AnswerPermission {
            request_id,
            session_id,
            pending_id,
            option_id,
        },
        AnswerRequest::Elicitation { action, content } => CollectorFrame::AnswerElicitation {
            request_id,
            session_id,
            pending_id,
            action,
            content,
        },
    }
}

/// One `pending` row joined with its answer, as read.
struct PendingRow {
    pending_id: String,
    session_id: String,
    kind: String,
    state: String,
    reason: Option<String>,
    turn_id: Option<String>,
    option_ids: Option<String>,
    payload: String,
    answered: bool,
    delivered: Option<bool>,
}

const PENDING_COLUMNS: &str = "p.pending_id, p.session_id, p.kind, p.state, p.reason, p.turn_id, p.option_ids,
     p.payload, q.pending_id IS NOT NULL, q.delivered
     FROM pending p LEFT JOIN answer_queue q ON q.pending_id = p.pending_id";

impl PendingRow {
    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            pending_id: r.get(0)?,
            session_id: r.get(1)?,
            kind: r.get(2)?,
            state: r.get(3)?,
            reason: r.get(4)?,
            turn_id: r.get(5)?,
            option_ids: r.get(6)?,
            payload: r.get(7)?,
            answered: r.get(8)?,
            delivered: r.get(9)?,
        })
    }

    fn item(self) -> Result<PendingItem> {
        Ok(PendingItem {
            pending_id: self.pending_id,
            session_id: self.session_id,
            kind: untag(self.kind)?,
            state: untag(self.state)?,
            reason: self.reason.map(untag).transpose()?,
            turn_id: self.turn_id,
            option_ids: self.option_ids.map(|ids| serde_json::from_str(&ids)).transpose()?,
            payload: serde_json::from_str(&self.payload)?,
            answered: self.answered,
            delivered: self.delivered,
        })
    }
}

/// Keep a stored host fact that did not apply as the idempotency key only:
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
        let ts = now();
        if let Some(turn) = open_turn.as_deref() {
            events.push(resolve_open_turn(tx, session_id, turn, &ts)?);
        }
        if !close_requested {
```

with:

```rust
        let ts = now();
        if let Some(turn) = open_turn.as_deref() {
            events.push(resolve_open_turn(tx, session_id, turn, &ts)?);
        }
        events.extend(cancel_open_pending(tx, session_id, PendingReason::SessionClosed, &ts)?);
        if !close_requested {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    /// A turn's state: `sent`, `started`, `ended` or `not_delivered`.
```

with:

```rust
    /// A session's open pending requests, oldest first (ACP core §9: the
    /// session detail).
    pub fn open_pending(&self, session_id: &str) -> Result<Vec<PendingItem>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {PENDING_COLUMNS} WHERE p.session_id = ?1 AND p.state = 'open' ORDER BY p.rowid"
        ))?;
        let rows = stmt.query_map([session_id], PendingRow::read)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.item()?);
        }
        Ok(out)
    }

    /// One pending request, whatever its state (SSE `pending_changed`).
    pub fn pending_item(&self, pending_id: &str) -> Result<Option<PendingItem>> {
        let row = self
            .conn()
            .query_row(
                &format!("SELECT {PENDING_COLUMNS} WHERE p.pending_id = ?1"),
                [pending_id],
                PendingRow::read,
            )
            .optional()?;
        row.map(PendingRow::item).transpose()
    }

    /// Accept an operator's answer to an open pending request of
    /// `session_id` (ACP core §4.6): validated against the stored kind and
    /// option ids, written as `answer_submitted` and queued durably. At most
    /// one answer per request: the check and the insert are one
    /// transaction, and the queue's key is the pending id.
    pub fn submit_answer(
        &self,
        session_id: &str,
        pending_id: &str,
        answer: &AnswerRequest,
    ) -> Result<AnswerSubmission> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, String, Option<String>, String, bool)> = tx
            .query_row(
                "SELECT p.kind, p.state, p.option_ids, s.host_id,
                        EXISTS(SELECT 1 FROM answer_queue q WHERE q.pending_id = p.pending_id)
                 FROM pending p JOIN sessions s ON s.id = p.session_id
                 WHERE p.pending_id = ?1 AND p.session_id = ?2",
                params![pending_id, session_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((kind, state, option_ids, host_id, queued)) = row else {
            return Ok(AnswerSubmission::NotFound);
        };
        if state != "open" {
            return Ok(AnswerSubmission::NotOpen);
        }
        if queued {
            return Ok(AnswerSubmission::AlreadyAnswered);
        }
        let option_ids: Option<Vec<String>> = option_ids.map(|ids| serde_json::from_str(&ids)).transpose()?;
        if let Err(why) = check_answer(untag(kind)?, option_ids.as_deref(), answer) {
            return Ok(AnswerSubmission::Invalid(why));
        }
        let request_id = uuid::Uuid::now_v7().to_string();
        let ts = now();
        tx.execute(
            "INSERT INTO answer_queue(pending_id, session_id, request_id, answer, submitted_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![pending_id, session_id, request_id, serde_json::to_string(answer)?, ts],
        )?;
        let body = json!({ "pending_id": pending_id, "request_id": request_id, "answer": answer });
        let event = collector_event(&tx, session_id, "answer_submitted", body, &ts)?;
        tx.commit()?;
        Ok(AnswerSubmission::Queued(Box::new(QueuedAnswer {
            event,
            host_id,
            frame: answer_frame(request_id.clone(), session_id, pending_id, answer.clone()),
            request_id,
        })))
    }

    /// The answers still owed to `host_id` (ACP core §4.6, §5.1): queued,
    /// with no verdict, for a question still open; oldest first. Sent after
    /// every handshake's reconciliation, so one lost with a connection goes
    /// again; the host dedupes by pending id.
    pub fn answers_to_send(&self, host_id: &str) -> Result<Vec<CollectorFrame>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT q.request_id, q.session_id, q.pending_id, q.answer
             FROM answer_queue q
                 JOIN pending p ON p.pending_id = q.pending_id
                 JOIN sessions s ON s.id = q.session_id
             WHERE s.host_id = ?1 AND q.delivered IS NULL AND p.state = 'open'
             ORDER BY q.rowid",
        )?;
        let rows = stmt.query_map([host_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (request_id, session_id, pending_id, answer) = row?;
            out.push(answer_frame(
                request_id,
                &session_id,
                &pending_id,
                serde_json::from_str(&answer)?,
            ));
        }
        Ok(out)
    }

    /// A turn's state: `sent`, `started`, `ended` or `not_delivered`.
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            SessionBody::SessionParked { .. } => {
                created.extend(release_turn_on_detach(&tx, session_id, &ts)?);
                // A park that overtakes an operator close ends the session
                // as the operator asked: closed.
                let changed = tx.execute(
                    "UPDATE sessions SET
                         lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                         activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::SessionClosed => {
                created.extend(release_turn_on_detach(&tx, session_id, &ts)?);
                // Also the host's confirmation of a close the collector
                // already made (an offline close): nothing left to change.
                let changed = tx.execute(
                    "UPDATE sessions SET lifecycle = 'closed', activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
```

with:

```rust
            SessionBody::SessionParked { reason } => {
                created.extend(release_turn_on_detach(&tx, session_id, &ts)?);
                // A park that overtakes an operator close ends the session
                // as the operator asked: closed.
                let changed = tx.execute(
                    "UPDATE sessions SET
                         lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                         activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    // The host cancels its questions before it detaches;
                    // one it left open goes with the session.
                    let reason = match reason {
                        ParkReason::AdapterExited => PendingReason::AdapterLost,
                        ParkReason::Idle | ParkReason::Operator => PendingReason::SessionParked,
                    };
                    created.extend(cancel_open_pending(&tx, session_id, reason, &ts)?);
                }
            }
            SessionBody::SessionClosed => {
                created.extend(release_turn_on_detach(&tx, session_id, &ts)?);
                // Also the host's confirmation of a close the collector
                // already made (an offline close): nothing left to change.
                let changed = tx.execute(
                    "UPDATE sessions SET lifecycle = 'closed', activity = NULL, close_requested = 0, presumed_parked = 0
                     WHERE id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1)",
                    [session_id],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    created.extend(cancel_open_pending(&tx, session_id, PendingReason::SessionClosed, &ts)?);
                }
            }
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
            // Diagnostics only, with no transition of their own: an
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing. Pending requests and answer verdicts
            // are stored the same way until the store keeps a pending set.
            SessionBody::AdapterExited { .. }
            | SessionBody::HostNote { .. }
            | SessionBody::PendingOpened { .. }
            | SessionBody::PendingResolved { .. }
            | SessionBody::AnswerResult { .. } => {
```

with:

```rust
            SessionBody::PendingOpened {
                pending_id,
                indexed,
                payload,
            } => {
                // A question of the attached session, asked in a turn that
                // is still open if it names one. Everything the collector
                // keeps comes from the extract (ACP core §3.2).
                let (lifecycle, presumed): (String, bool) = tx.query_row(
                    "SELECT lifecycle, presumed_parked FROM sessions WHERE id = ?1",
                    [session_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                let extract = indexed.pending.as_ref().filter(|p| p.id == *pending_id);
                let applies =
                    (lifecycle == "active" || presumed) && fact_applies(&tx, session_id, indexed.turn_id.as_deref())?;
                let inserted = match extract.filter(|_| applies) {
                    Some(extract) => tx.execute(
                        "INSERT INTO pending(pending_id, session_id, kind, turn_id, option_ids, payload, state, opened_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7)
                         ON CONFLICT(pending_id) DO NOTHING",
                        params![
                            pending_id,
                            session_id,
                            tag(extract.kind)?,
                            indexed.turn_id,
                            extract.option_ids.as_ref().map(serde_json::to_string).transpose()?,
                            payload.to_string(),
                            ts
                        ],
                    )?,
                    None => 0,
                };
                if inserted == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                } else {
                    tx.execute(
                        "UPDATE sessions SET activity = 'blocked' WHERE id = ?1 AND activity = 'running'",
                        [session_id],
                    )?;
                }
            }
            SessionBody::PendingResolved {
                pending_id,
                resolution,
                reason,
            } => {
                let state = match resolution {
                    PendingResolution::Delivered => PendingState::Delivered,
                    PendingResolution::Cancelled => PendingState::Cancelled,
                };
                if !resolve_pending(&tx, session_id, pending_id, state, *reason, &ts)? {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            SessionBody::AnswerResult {
                pending_id, delivered, ..
            } => {
                // Folded monotonically: `delivered` sticks, a later `false`
                // never overwrites it (umbrella §6.8).
                let changed = tx.execute(
                    "UPDATE answer_queue SET delivered = CASE WHEN delivered = 1 THEN 1 ELSE ?3 END
                     WHERE pending_id = ?1 AND session_id = ?2",
                    params![pending_id, session_id, delivered],
                )?;
                if changed == 0 {
                    created.clear();
                    mark_unapplied(&tx, fact_id)?;
                }
            }
            // Diagnostics only, with no transition of their own: an
            // `adapter_exited` is followed by the `session_parked` that
            // detaches; a `host_note` (e.g. `replay_unknown_dropped` after a
            // load) changes nothing.
            SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } => {
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
                    if host.is_none() {
                        tx.execute(
                            "UPDATE sessions SET
                                 lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
```

with:

```rust
                    if host.is_none() {
                        // The restarted host holds none of its questions.
                        out.events
                            .extend(cancel_open_pending(&tx, &id, PendingReason::HostRestarted, &ts)?);
                        tx.execute(
                            "UPDATE sessions SET
                                 lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all 293 tests pass, including the seven new `store` tests.

Check that the fold is a real guard. In the `AnswerResult` arm, replace `CASE WHEN delivered = 1 THEN 1 ELSE ?3 END` with `?3`: `a_delivered_verdict_sticks_and_a_later_false_does_not_overwrite_it` fails with `Some(false)`. Restore it.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(sessions): the pending set and the durable answer queue"
git push
```

### Task 6: The answer endpoint, the drain, and `pending_changed`

**Files:**
- Modify: `crates/hennery-sessions/src/hub.rs`, `crates/hennery-sessions/src/ws.rs`, `crates/hennery-sessions/src/api.rs`
- Test: `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes (Task 5): `Store::{submit_answer, answers_to_send, open_pending, pending_item}`, `AnswerSubmission`, `QueuedAnswer`.
- Produces: `Hub::notify(&self, host_id: &str, frame: CollectorFrame) -> bool`. It sends a frame nobody waits for to a host that is connected and reconciled, and returns `false` otherwise.
- Produces (`ws.rs`):
  - After `mark_ready`, the reconciliation sends `answers_to_send`. A store error there drops the connection, like a failed reconciliation.
  - A host `error` whose request id is neither a waiter's nor a reconcile close's is logged. For an answer it is no verdict (decision 10).
- Produces (`api.rs`):
  - `POST /api/sessions/{id}/pending/{pending_id}/answer` (decision 11);
  - `SessionDetail.pending` from `open_pending`;
  - `sse_messages(store, event)` adds `pending_changed` for the five question event kinds (decision 12).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/reconcile.rs`:

```rust
// Plan (2): answers through the API, the queue and the host (ACP core §4.6,
// §5, §9).

fn opened(pending_id: &str, turn_id: &str) -> SessionBody {
    use hennery_proto::frames::{Indexed, PendingExtract, PendingKind};
    SessionBody::PendingOpened {
        pending_id: pending_id.into(),
        indexed: Indexed {
            turn_id: Some(turn_id.into()),
            pending: Some(PendingExtract {
                id: pending_id.into(),
                kind: PendingKind::Permission,
                option_ids: Some(vec!["allow".into(), "reject".into()]),
            }),
            ..Indexed::default()
        },
        payload: json!({"toolCall": {"toolCallId": "call-1"}}),
    }
}

fn answer_url(collector: &Collector, session: &str, pending_id: &str) -> String {
    collector.url(&format!("/api/sessions/{session}/pending/{pending_id}/answer"))
}

/// A session with turn `t` running and question `p1` open in it.
async fn asking_session(collector: &Collector, host: &mut ScriptedHost) -> (String, String) {
    let session = started_session(collector, host).await;
    let turn = started_turn(collector, host, &session).await;
    host.emit(&session, opened("p1", &turn)).await;
    wait_for("the question", || async {
        (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
    })
    .await;
    (session, turn)
}

/// The next frame must be `p1`'s answer; returns its request id.
async fn expect_answer(host: &mut ScriptedHost, session: &str, option: &str) -> String {
    match host.next().await {
        CollectorFrame::AnswerPermission {
            request_id,
            session_id,
            pending_id,
            option_id,
        } => {
            assert_eq!(
                (session_id.as_str(), pending_id.as_str(), option_id.as_str()),
                (session, "p1", option)
            );
            request_id
        }
        other => panic!("expected an answer, got {other:?}"),
    }
}

fn verdict_of(collector: &Collector) -> Option<bool> {
    collector.state.store.pending_item("p1").unwrap().unwrap().delivered
}

#[tokio::test]
async fn an_answer_is_queued_sent_to_the_host_and_its_verdict_recorded() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    let detail_url = collector.url(&format!("/api/sessions/{session}"));
    let (_, detail) = get(&client(), detail_url.clone()).await;
    assert_eq!(detail["activity"], "blocked");
    assert_eq!(detail["pending"][0]["pending_id"], "p1");
    assert_eq!(detail["pending"][0]["option_ids"], json!(["allow", "reject"]));

    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    let request_id = expect_answer(&mut host, &session, "allow").await;
    assert_eq!(body, json!({"pending_id": "p1", "request_id": request_id}));
    // One answer per question, even before its verdict.
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "reject"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("already_answered")));

    host.emit(
        &session,
        SessionBody::AnswerResult {
            pending_id: "p1".into(),
            request_id,
            delivered: true,
        },
    )
    .await;
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p1".into(),
            resolution: hennery_proto::frames::PendingResolution::Delivered,
            reason: None,
        },
    )
    .await;
    wait_for("delivered", || async {
        (verdict_of(&collector) == Some(true)).then_some(())
    })
    .await;
    let (_, detail) = get(&client(), detail_url).await;
    assert_eq!(
        (detail["activity"].as_str(), &detail["pending"]),
        (Some("running"), &json!([]))
    );
    let (status, body) = post(&client(), url, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

#[tokio::test]
async fn answers_are_checked_against_the_stored_request() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "maybe"})).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let (status, _) = post(&client(), url.clone(), json!({"action": "accept"})).await;
    assert_eq!(status, 400, "an elicitation's answer to a permission request");
    let (status, _) = post(&client(), url, json!({"action": "whatever"})).await;
    assert_eq!(status, 422);
    let (status, body) = post(
        &client(),
        answer_url(&collector, &session, "no-such-question"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (404, Some("not_found")));
    assert!(verdict_of(&collector).is_none(), "nothing was queued");
}

/// Scenario 9: an answer given while the host is away is delivered after
/// its next handshake, and again after the one after that for as long as
/// no verdict came (the host dedupes by pending id).
#[tokio::test]
async fn an_answer_given_while_the_host_is_offline_is_sent_after_its_next_handshake() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, turn) = asking_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let (status, body) = post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");

    let listed = || {
        vec![AttachedSession {
            session_id: session.clone(),
            last_seq: seq,
            open_turn_id: Some(turn.clone()),
        }]
    };
    let mut host = ScriptedHost::hello(&collector, listed(), seq).await;
    // Nothing before the reconciliation.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), host.next())
            .await
            .is_err(),
        "sent before resend_complete"
    );
    host.send(&HostFrame::ResendComplete).await;
    let first = expect_answer(&mut host, &session, "allow").await;
    host.drop_connection(&collector).await;
    let mut host = ScriptedHost::connect(&collector, listed(), seq).await;
    assert_eq!(expect_answer(&mut host, &session, "allow").await, first, "resent as is");
    assert!(verdict_of(&collector).is_none());
}

/// A host's refusal (no live actor for the session) is logged, not a
/// verdict: the answer waits for its question's resolution.
#[tokio::test]
async fn a_refused_answer_gets_its_verdict_from_its_questions_resolution() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let request_id = expect_answer(&mut host, &session, "allow").await;
    host.send(&HostFrame::Error {
        request_id,
        code: "not_attached".into(),
        message: "session is not attached on this host".into(),
    })
    .await;
    // Frames are read in order: once this note is in, so is the refusal.
    host.emit(
        &session,
        SessionBody::HostNote {
            note: "marker".into(),
            text: String::new(),
        },
    )
    .await;
    wait_for("the marker", || async {
        collector
            .event_kinds(&session)
            .contains(&"host_note".to_string())
            .then_some(())
    })
    .await;
    assert_eq!(verdict_of(&collector), None, "a refusal is logged, not a verdict");
    host.emit(
        &session,
        SessionBody::PendingResolved {
            pending_id: "p1".into(),
            resolution: hennery_proto::frames::PendingResolution::Cancelled,
            reason: Some(hennery_proto::frames::PendingReason::AdapterLost),
        },
    )
    .await;
    wait_for("the verdict", || async {
        (verdict_of(&collector) == Some(false)).then_some(())
    })
    .await;
}

/// Scenario 7's questions: a restarted host holds none, so they are
/// cancelled after its resend, and an answer queued for one is never sent.
#[tokio::test]
async fn a_host_restart_cancels_the_open_questions_and_drops_their_queued_answers() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    host.drop_connection(&collector).await;
    post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    assert_eq!(collector.lifecycle(&session), "parked");
    let item = collector.state.store.pending_item("p1").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason, item.delivered)).unwrap(),
        json!(["cancelled", "host_restarted", false])
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), host.next())
            .await
            .is_err(),
        "an answer to a cancelled question was sent"
    );
}

#[tokio::test]
async fn every_step_of_a_question_is_a_pending_changed_message_on_the_session_stream() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let request_id = expect_answer(&mut host, &session, "allow").await;
    host.emit(
        &session,
        SessionBody::AnswerResult {
            pending_id: "p1".into(),
            request_id,
            delivered: true,
        },
    )
    .await;
    let stream = read_stream(&collector, &session, |s| {
        s.matches("event: pending_changed").count() >= 3
    })
    .await;
    let messages: Vec<&str> = stream.split("\n\n").collect();
    let id = |m: &str| m.lines().find(|l| l.starts_with("id: ")).map(str::to_string);
    for kind in ["pending_opened", "answer_submitted", "answer_result"] {
        let at = messages
            .iter()
            .position(|m| m.contains("event: event") && m.contains(&format!(r#""kind":"{kind}""#)))
            .unwrap_or_else(|| panic!("no {kind} in {stream}"));
        let changed = messages[at + 1];
        assert!(changed.contains("event: pending_changed"), "{changed}");
        assert_eq!(id(changed), id(messages[at]), "pending_changed carries its event's id");
        assert!(changed.contains(r#""pending_id":"p1""#), "{changed}");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p hennery-testkit --test reconcile --locked`
Expected: 6 FAIL (33 pass):
- the answer endpoint does not exist yet, so posting to it answers 404 (`left: (404, None)`);
- the detail's `pending` is `[]`, so the check that it lists `p1` fails (`left: Null`);
- the tests waiting for an answer frame time out (`a collector frame within 10s`).

- [ ] **Step 3: Answer through the API, drain after reconciliation, stream every step**

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    /// Send a request and wait until the outboxed fact carrying `request_id`
```

with:

```rust
    /// Send a frame nobody waits for (an answer: its verdict arrives as a
    /// fact, ACP core §4.6) to a host that is connected and reconciled.
    /// `false` if it is not: the frame then goes after its next handshake.
    pub fn notify(&self, host_id: &str, frame: CollectorFrame) -> bool {
        self.hosts
            .lock()
            .expect("hosts lock")
            .get(host_id)
            .filter(|h| h.ready)
            .is_some_and(|h| h.tx.send(frame).is_ok())
    }

    /// Send a request and wait until the outboxed fact carrying `request_id`
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
                        for session_id in done.close {
                            let request_id = uuid::Uuid::now_v7().to_string();
                            reconcile_closes.insert(request_id.clone(), session_id.clone());
                            let _ = tx.send(CollectorFrame::CloseSession { request_id, session_id });
                        }
                        reconciled = true;
                        state.hub.mark_ready(&host_id, conn_id);
                        tracing::info!(%host_id, "host reconciled");
```

with:

```rust
                        for session_id in done.close {
                            let request_id = uuid::Uuid::now_v7().to_string();
                            reconcile_closes.insert(request_id.clone(), session_id.clone());
                            let _ = tx.send(CollectorFrame::CloseSession { request_id, session_id });
                        }
                        reconciled = true;
                        state.hub.mark_ready(&host_id, conn_id);
                        // The answer queue (ACP core §5.1 step 4), after the
                        // reconciliation cancelled what a restart lost, and
                        // after `mark_ready`: an answer submitted meanwhile
                        // is either read here or sent by its own handler
                        // (or both; the host dedupes by pending id).
                        match state.store.answers_to_send(&host_id) {
                            Ok(answers) => {
                                for frame in answers {
                                    let _ = tx.send(frame);
                                }
                            }
                            Err(err) => {
                                tracing::error!(%host_id, error = %err, "reading the answer queue failed; dropping connection");
                                break;
                            }
                        }
                        tracing::info!(%host_id, "host reconciled");
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
                        match undo_failed {
                            None => rejection.answer(code, message),
                            Some((undo, err)) => {
                                tracing::error!(%host_id, ?undo, error = %err, "undoing a rejected request failed; dropping connection");
                                rejection.delivery_unknown();
                                break;
                            }
                        }
                    }
```

with:

```rust
                        match undo_failed {
                            None => rejection.answer(code, message),
                            Some((undo, err)) => {
                                tracing::error!(%host_id, ?undo, error = %err, "undoing a rejected request failed; dropping connection");
                                rejection.delivery_unknown();
                                break;
                            }
                        }
                    } else {
                        // No waiter: an answer (its verdict comes from
                        // `answer_result` or its question's resolution),
                        // or a request whose caller already gave up.
                        tracing::warn!(%host_id, %request_id, %code, %message, "host refused a request nobody waits for");
                    }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use crate::store::ResumeRequest;
```

with:

```rust
use crate::store::{AnswerSubmission, ResumeRequest, Store};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use hennery_proto::rest::{
    ApiError, CancelResponse, ConfigRequest, EventDto, LifecycleResponse, OpenTurn, PromptRequest, PromptResponse,
    SessionCatalog, SessionDetail, StartSessionRequest, StartSessionResponse,
};
```

with:

```rust
use hennery_proto::rest::{
    AnswerRequest, AnswerResponse, ApiError, CancelResponse, ConfigRequest, EventDto, LifecycleResponse, OpenTurn,
    PendingItem, PromptRequest, PromptResponse, SessionCatalog, SessionDetail, StartSessionRequest,
    StartSessionResponse,
};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .route("/api/sessions/{id}/config", post(set_config))
```

with:

```rust
        .route("/api/sessions/{id}/config", post(set_config))
        .route("/api/sessions/{id}/pending/{pending_id}/answer", post(answer))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        None => None,
    };
    Json(SessionDetail {
```

with:

```rust
        None => None,
    };
    let pending = match state.store.open_pending(&id) {
        Ok(pending) => pending,
        Err(err) => return internal(err),
    };
    Json(SessionDetail {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        presumed_parked: session.presumed_parked,
        open_turn,
        pending: Vec::new(),
    })
    .into_response()
```

with:

```rust
        presumed_parked: session.presumed_parked,
        open_turn,
        pending,
    })
    .into_response()
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

with:

```rust
/// Answer a pending request (ACP core §4.6, §9): 202 once the answer is
/// queued durably, whatever the host's state. It goes out now if the host
/// is connected and reconciled, else after its next handshake; the verdict
/// follows as SSE `pending_changed`.
async fn answer(
    State(state): State<AppState>,
    Path((id, pending_id)): Path<(String, String)>,
    Json(req): Json<AnswerRequest>,
) -> Response {
    match state.store.submit_answer(&id, &pending_id, &req) {
        Ok(AnswerSubmission::Queued(queued)) => {
            state.hub.publish(queued.event);
            state.hub.notify(&queued.host_id, queued.frame);
            let body = AnswerResponse {
                pending_id,
                request_id: queued.request_id,
            };
            (StatusCode::ACCEPTED, Json(body)).into_response()
        }
        Ok(AnswerSubmission::NotFound) => error(StatusCode::NOT_FOUND, "not_found", "no such pending request"),
        Ok(AnswerSubmission::NotOpen) => error(
            StatusCode::CONFLICT,
            "not_open",
            "the request was answered or cancelled already",
        ),
        Ok(AnswerSubmission::AlreadyAnswered) => error(
            StatusCode::CONFLICT,
            "already_answered",
            "an answer is already on its way",
        ),
        Ok(AnswerSubmission::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

fn lifecycle_response(state: &AppState, id: &str) -> Response {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
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
```

with:

```rust
/// The SSE messages for one stored event: the event, then `catalog_changed`
/// with the same id if it carries a catalogue snapshot, and
/// `pending_changed` with the same id if it concerns a pending request
/// (ACP core §9). A listed event with a snapshot is one that changed the
/// stored catalogue, and both come from the stored row, so a replay from
/// `Last-Event-ID` sends them too. `pending_changed` carries the request as
/// it stands when the message is sent.
fn sse_messages(store: &Store, e: &EventDto) -> Vec<Result<Event, Infallible>> {
    let mut out = vec![Ok(sse_event(e))];
    if let Some(catalog) = catalog_in(e) {
        out.push(Ok(Event::default()
            .id(e.event_id.to_string())
            .event("catalog_changed")
            .data(serde_json::to_string(&catalog).expect("catalog serializes"))));
    }
    if let Some(pending) = pending_in(store, e) {
        out.push(Ok(Event::default()
            .id(e.event_id.to_string())
            .event("pending_changed")
            .data(serde_json::to_string(&pending).expect("pending request serializes"))));
    }
    out
}

/// The pending request a stored event concerns, as it stands now.
fn pending_in(store: &Store, e: &EventDto) -> Option<PendingItem> {
    if !matches!(
        e.kind.as_str(),
        "pending_opened" | "pending_resolved" | "pending_cancelled" | "answer_submitted" | "answer_result"
    ) {
        return None;
    }
    let pending_id = e.body.get("pending_id")?.as_str()?;
    store.pending_item(pending_id).ok().flatten()
}
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    let replay = stream::iter(backlog.iter().flat_map(sse_messages).collect::<Vec<_>>());
    let session = id.clone();
    let follow = live
        .filter_map(move |item| {
            let session = session.clone();
            async move {
                match item {
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&e)),
```

with:

```rust
    let replay = stream::iter(
        backlog
            .iter()
            .flat_map(|e| sse_messages(&state.store, e))
            .collect::<Vec<_>>(),
    );
    let session = id.clone();
    let store = state.store.clone();
    let follow = live
        .filter_map(move |item| {
            let session = session.clone();
            let store = store.clone();
            async move {
                match item {
                    Ok(e) if e.session_id == session && e.event_id > last => Some(sse_messages(&store, &e)),
```

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all 299 tests pass, including the six new `reconcile` tests. `an_answer_given_while_the_host_is_offline_is_sent_after_its_next_handshake` also checks that nothing is sent before `resend_complete`. `a_host_restart_cancels_the_open_questions_and_drops_their_queued_answers` checks that the drain runs after the reconciliation's cancellations. Each asserts 300 ms of silence, which holds under load: the thing it rules out would arrive at once.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(sessions): answer endpoint, queue drain after reconciliation and pending_changed"
git push
```

### Task 7: End to end: questions, answers and their cancellations

**Files:**
- Test: `crates/hennery-testkit/tests/e2e.rs`

**Interfaces:**
- Consumes: everything above, over a real collector, a real host and the fake adapter as a child process.
- Produces: `Collector::start_with(db, addr, offline_threshold)` (a test helper), and these tests:
  - both kinds of question answered through the API (the elicitation live gate against the fake: the form content reaches the agent and `answer_result{delivered: true}`);
  - a stop with a question open;
  - scenario 5 (adapter crash) and scenario 7 (host restart) with a question open;
  - scenarios 8 and 9 together: the host is away past the offline threshold, the question stays open, the answer given meanwhile is delivered after the reconnect, and the adapter is still waiting for it.

This task adds no production code. Its tests pass against Tasks 1-6. Step 2 therefore removes one guard, to show that they are real guards.

- [ ] **Step 1: Write the tests**

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
impl Collector {
    async fn start(db: &Path, addr: Option<SocketAddr>) -> Self {
        let listener = tokio::net::TcpListener::bind(addr.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()))
            .await
            .expect("bind collector");
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(Store::open(db).unwrap(), DevToken::new(TOKEN));
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, task }
    }
```

with:

```rust
impl Collector {
    async fn start(db: &Path, addr: Option<SocketAddr>) -> Self {
        Self::start_with(db, addr, hennery_sessions::offline::OFFLINE_THRESHOLD).await
    }

    /// A collector that presumes a host's sessions parked once it has been
    /// offline for `offline`.
    async fn start_with(db: &Path, addr: Option<SocketAddr>, offline: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind(addr.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()))
            .await
            .expect("bind collector");
        let addr = listener.local_addr().unwrap();
        let mut state = AppState::new(Store::open(db).unwrap(), DevToken::new(TOKEN));
        state.offline_threshold = offline;
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, task }
    }
```

Append to `crates/hennery-testkit/tests/e2e.rs`:

```rust
// Plan (2): permission and elicitation end to end (ACP core §4.6, §12
// scenarios 5 and 7 to 10, and the elicitation live gate against the fake).

fn asking(asks: Vec<hennery_testkit::FakeAsk>) -> FakeScript {
    FakeScript {
        asks,
        ..FakeScript::default()
    }
}

async fn detail(c: &reqwest::Client, collector: &Collector, session: &str) -> Value {
    let resp = c
        .get(collector.url(&format!("/api/sessions/{session}")))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

/// The id of the question the session is blocked on, once it is.
async fn open_question(c: &reqwest::Client, collector: &Collector, session: &str) -> String {
    wait_for("an open question", || async {
        let detail = detail(c, collector, session).await;
        let pending = detail["pending"].as_array()?.first()?.clone();
        (detail["activity"] == "blocked").then(|| pending["pending_id"].as_str().unwrap().to_string())
    })
    .await
}

async fn answer(c: &reqwest::Client, collector: &Collector, session: &str, pending: &str, body: Value) -> (u16, Value) {
    let url = collector.url(&format!("/api/sessions/{session}/pending/{pending}/answer"));
    post_json(c, url, body).await
}

async fn prompt(c: &reqwest::Client, collector: &Collector, session: &str) {
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
    let (status, body) = post_json(c, url, json!({ "content": text("go") })).await;
    assert_eq!(status, 202, "{body}");
}

fn delivered(collector: &Collector, pending: &str) -> Option<bool> {
    collector.state.store.pending_item(pending).unwrap().unwrap().delivered
}

/// Both kinds of question through the API, and the elicitation gate: the
/// form's content reaches the agent and the answer is reported delivered.
#[tokio::test]
async fn questions_answered_through_the_api_reach_the_agent_and_are_reported_delivered() {
    use hennery_testkit::FakeAsk;
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = asking(vec![FakeAsk::Permission, FakeAsk::Elicitation]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;

    let first = open_question(&c, &collector, &session).await;
    let (status, body) = answer(&c, &collector, &session, &first, json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    let second = wait_for("the second question", || async {
        let detail = detail(&c, &collector, &session).await;
        let id = detail["pending"].as_array()?.first()?["pending_id"]
            .as_str()?
            .to_string();
        (id != first).then_some(id)
    })
    .await;
    let form = json!({"action": "accept", "content": {"name": "notes.txt"}});
    assert_eq!(answer(&c, &collector, &session, &second, form).await.0, 202);

    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert_eq!(
        agent_text(&evs),
        r#"permission:selected:allowelicitation:accept:{"name":"notes.txt"}Hello world"#
    );
    let results = of_kind(&evs, "answer_result");
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|e| e.body["delivered"] == true), "{results:?}");
    assert_eq!(
        (delivered(&collector, &first), delivered(&collector, &second)),
        (Some(true), Some(true))
    );
    let detail = detail(&c, &collector, &session).await;
    assert_eq!(
        (detail["activity"].as_str(), &detail["pending"]),
        (Some("idle"), &json!([]))
    );
}

#[tokio::test]
async fn stop_with_a_question_open_ends_the_turn_cancelled_and_closes_the_question() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = asking(vec![hennery_testkit::FakeAsk::Permission]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;

    let url = collector.url(&format!("/api/sessions/{session}/cancel"));
    let (status, body) = post_json(&c, url, json!({})).await;
    assert_eq!((status, body["outcome"].as_str()), (202, Some("cancelled")), "{body}");
    let item = collector.state.store.pending_item(&pending).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason)).unwrap(),
        json!(["cancelled", "turn_cancelled"])
    );
    let (status, body) = answer(&c, &collector, &session, &pending, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
    assert!(agent_text(&events(&c, &collector, &session).await).contains("permission:cancelled"));
}

/// Scenario 5, with a question open.
#[tokio::test]
async fn an_adapter_crash_with_a_question_open_cancels_it_adapter_lost() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = FakeScript {
        crash_while_asking: true,
        ..asking(vec![hennery_testkit::FakeAsk::Permission])
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    lifecycle_is(&collector, &session, "parked").await;

    let evs = events(&c, &collector, &session).await;
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    let at = kinds
        .iter()
        .position(|k| *k == "pending_opened")
        .expect("the question was recorded");
    assert_eq!(
        kinds[at..],
        [
            "pending_opened",
            "turn_ended",
            "pending_resolved",
            "adapter_exited",
            "session_parked"
        ]
    );
    assert_eq!(evs[at + 2].body["reason"], "adapter_lost");
    let pending = evs[at].body["pending_id"].as_str().unwrap();
    let (status, body) = answer(&c, &collector, &session, pending, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

/// Scenario 7, with a question open: the restarted host holds none, so it
/// is cancelled after the resend, and nothing is re-spawned.
#[tokio::test]
async fn a_host_restart_with_a_question_open_cancels_it_host_restarted() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let script = asking(vec![hennery_testkit::FakeAsk::Permission]);
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;

    host.abort();
    let _ = host.await;
    start_host_with(collector.addr, &dir.path().join("host"), fake);
    lifecycle_is(&collector, &session, "parked").await;
    let evs = events(&c, &collector, &session).await;
    let cancelled = of_kind(&evs, "pending_cancelled");
    assert_eq!(cancelled.len(), 1, "{evs:?}");
    assert_eq!(
        cancelled[0].body,
        json!({"pending_id": pending, "reason": "host_restarted"})
    );
    let (status, body) = answer(&c, &collector, &session, &pending, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
}

/// Scenarios 8 and 9: while its host is away past the offline threshold
/// the session is presumed parked with its question still open, and an
/// answer given then is delivered once the host is back, the adapter still
/// waiting for it.
#[tokio::test]
async fn a_question_outlasts_its_host_being_away_and_an_answer_given_meanwhile_is_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let collector = Collector::start(&db, None).await;
    let addr = collector.addr;
    start_host(
        addr,
        &dir.path().join("host"),
        &asking(vec![hennery_testkit::FakeAsk::Permission]),
    );
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt(&c, &collector, &session).await;
    let pending = open_question(&c, &collector, &session).await;
    collector.stop().await;

    // A collector the host cannot reach (another address) presumes its
    // sessions parked, and takes the answer.
    let away = Collector::start_with(&db, None, Duration::from_millis(200)).await;
    wait_for("presumed parked", || async {
        let row = away.state.store.session(&session).unwrap().unwrap();
        (row.lifecycle == "parked" && row.presumed_parked).then_some(())
    })
    .await;
    let item = away.state.store.pending_item(&pending).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(item.state).unwrap(),
        "open",
        "the host may still hold it"
    );
    let (status, body) = answer(&c, &away, &session, &pending, json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
    away.stop().await;

    // Back where the host looks for it: reattached, then the queue drains.
    let collector = Collector::start(&db, Some(addr)).await;
    lifecycle_is(&collector, &session, "active").await;
    let evs = wait_for("turn end", || async {
        let evs = events(&c, &collector, &session).await;
        (!turn_ends(&evs).is_empty()).then_some(evs)
    })
    .await;
    assert!(!of_kind(&evs, "reattached").is_empty());
    assert!(
        agent_text(&evs).starts_with("permission:selected:allow"),
        "{}",
        agent_text(&evs)
    );
    assert_eq!(delivered(&collector, &pending), Some(true));
}
```

- [ ] **Step 2: Run them, and watch the scenario 8/9 test fail without the drain**

Run: `cargo test -p hennery-testkit --test e2e --locked`
Expected: all 25 pass.

In `crates/hennery-sessions/src/ws.rs`, replace the drain's `for frame in answers { let _ = tx.send(frame); }` with `drop(answers);`. Then run `cargo test -p hennery-testkit --test e2e --locked a_question_outlasts`. Expected: FAIL, `timed out waiting for turn end`, because the answer queued while the host was away never goes out. Restore the loop.

- [ ] **Step 3: Run everything**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 304 tests pass; `--check` exits 0.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "test(e2e): questions, answers and their cancellations end to end"
git push
```

## After this plan

**Obligations plan (2) hands on:**
- **The frontend's question cards.**
  - They are driven by `SessionDetail.pending` and SSE `pending_changed`, never by timeline position (§4.6).
  - A card shows "answered" only once `delivered: true`, and shows each cancellation reason explicitly: "the agent is no longer waiting, resume to continue" (umbrella §6.5).
  - A permission card is rendered from the ACP payload's `toolCall` and `options`; an elicitation form from `requestedSchema`.
  - The frontend plan settles the TS optionals of `PendingItem`, as for the rest of `rest.rs`.
- **Push for `activity → blocked`** (§10). The edge is the ingest of the `pending_opened` that sets `blocked` (decision 8). Recovery and reconciliation must not push it.
- **Push for a question asked outside a turn.** `activity` stays `idle` then (decision 8), so there is no "needs your answer" edge. The push plan must trigger on the `pending_opened` itself for such a question.
- **Host revoke must cancel open questions** (kernel §4.3). `presumed_parked{host_revoked}` keeps them `open` like an offline presumption, but a revoked host never reconnects, so nothing would ever cancel them. The real-auth plan cancels them with a new reason (`host_revoked`, for the spec's list).
- **`elicitation/complete` is swallowed today.** The notification handler ignores everything but `session/update`. That is fine while URL-mode elicitation is not advertised; the plan that advertises it must forward the notification and resolve its question.
- **`fs/*` and `terminal/*`** (§2.5, §6). Until they exist, the host answers `-32601`, so an adapter that relies on them fails the tool call instead of hanging. Each needs its own arm in the request handler, ahead of the `-32601` fallback.
- **A question that blocks a start.** An adapter that awaits an answer inside `session/new` or `session/load` (an MCP server asking for OAuth during session setup, for example) waits for a question that opens only after `session_started`, so the start fails after 75 s, naming the questions (decision 2). If a real adapter does this, open such questions before `session_started`. The collector would then need to accept a `pending_opened` on a `starting` session.
- **Delete** (§4.10) must also delete the session's `pending` and `answer_queue` rows.
- **Live gate with real adapters:** the form elicitation round trip with `answer_result{delivered: true}` (§12) still needs a logged-in CI account and the pinned adapters. `questions_answered_through_the_api_reach_the_agent_and_are_reported_delivered` is its stand-in against the fake.
- **Spec amendments** listed under the decisions.
- **Wall-clock budgets in older tests.** Seven tests from earlier plans assume a CPU that is not overloaded:
  - `a_prompt_that_fails_because_the_adapter_is_dying_ends_interrupted` (`EXIT_SETTLE`);
  - `a_cancel_is_read_promptly_even_though_a_switch_deadline_fires_mid_flood`, `a_flooding_adapter_cannot_hold_off_a_cancel`;
  - `a_silent_connection_is_dropped_and_reconnected_within_the_read_deadline`;
  - `backoff_*`, `hello_reports_live_sessions_…`, `a_collector_that_never_completes_the_handshake_is_retried`.

  They fail on `7e5bcc1` (the B2b head, content-identical to `4659c27`) itself when sixteen full test binaries run at once on a loaded machine. Four copies of one binary, the CI-like load, pass. They are not affected by this plan. Hold them with the `test-hooks` seam, or poll, when they next need touching.
- **Carried from B2b, unchanged:**
  - legacy model and mode switching;
  - the New-session pickers before a session exists;
  - the rest of the catalogue;
  - `model` / `mode` in the list and detail items;
  - an adapter that answers switches without a catalogue;
  - a timed-out live switch that still lands;
  - an adapter that exits with a switch out answers it `config_failed`;
  - a cancelled turn's end waits for the whole Inbound backlog (the unbounded drain);
  - the flaky CLI test `sigint_to_ups_process_group_still_shuts_down_cleanly`;
  - B2a's `images` / `projects` capabilities, the fixed `CANCEL_GRACE`, and the `hello.capabilities` doc.

Then, in order (unchanged):
- **(3) Real auth and pairing.**
- **(4) Frontend shell**, including the question cards above.
- **(5) Hats.**
- **(6) Gateway.**
- **(7) Distribution.**

---

_Generated with Claude AI — please review before distribution._
