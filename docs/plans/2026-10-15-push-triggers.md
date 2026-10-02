# Web Push (plan 10b-i): the push triggers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the collector decides, from session state, when to notify the owner, and with what (ACP core §10; umbrella §1.2, "the agent asks, I answer from my phone"):
- a turn blocked on a question: "needs your answer", urgent;
- a turn that ended `completed`: "finished";
- a turn that ended `failed`: "failed".

Each notice goes to a queue that never blocks the host's ingest. Delivery (plan 10b-ii) drains the queue, applies the hat's policy and sends. Until then the collector's queue has no reader, and notices are dropped.

**Architecture:**
- **Wire** (`hennery-proto`): `PendingExtract::title`, the question's title, and `Indexed::pending` boxed.
- **Host** (`hennery-host`): `question_title`: a permission's `toolCall.title`, an elicitation's `message`, cut to 200 characters.
- **Store** (`hennery-sessions`):
  - `Store::ingest_fact(..) -> Ingested { events, edge: Option<Edge> }`;
  - `Edge { kind: PushEdge, session: EdgeSession }`, with `PushEdge::{Blocked, QuestionOutsideTurn, TurnEnded}` read from what the fact changed, and the session's id, hat, title and cwd, all in the fact's own transaction;
  - `still_blocked_on(session, pending)`;
  - `ingest` keeps its signature (the events only).
- **Kernel** (`hennery-kernel`): `push::Notice`, `Urgency`, `Push` and `Notices`.
  - `Push::new` makes a queue of 256 tags, each holding its latest notice, and its reading end.
  - `notify` never waits and never fails. `detached` makes a queue nobody reads.
- **Triggers** (`hennery-sessions`):
  - `notify::notice_for(edge, session) -> Option<Notice>` is the one place that decides.
  - `ws.rs` calls it after a host fact's commit, and nowhere else. Edges from a resend wait until reconciliation and must still hold then.
  - The queue is reached through `AppState::push`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md) §10 (push triggers) and §4.2, §4.4; the kernel spec's §6:
- ACP core §10: "Evaluated on ingest, edge-triggered only: activity → `blocked`; `turn_ended{completed}`; `turn_ended{failed}` or `agent_failure` (severity ≠ warning)". "Title is the session title, falling back to the project directory name." "The payload carries `url: /sessions/<id>`." "Recovery and reconciliation never push, and a `turn_ended` for an already ended turn never pushes."
- kernel §6: "urgency `high` for 'needs your answer', `normal` otherwise". The policy `details` includes "the agent's question title".
- umbrella §5: "A question the agent asks outside a turn is pending too, but leaves the activity alone (ACP core §4.2)."

It builds on plan [10a](2026-10-14-push.md) (the VAPID key, subscriptions, hat policies), on plan (2)'s pending questions and on plan A's teardown. Every anchor was taken from `plan/push` at `6855342` (10a, PR #60).

**Status:** written 2026-10-02; executed 2026-10-02 (see "Execution status"). Amended after:
- the security review of 2026-10-02 (A1–A5; O1, O2 recorded);
- its scoped re-confirmation, "confirmed" with A6 (taken) and N1, N2 recorded;
- the task review.

**How the code blocks were made and checked:**
- The code was built and tested first; every block below was generated from its diff.
- The plan was replayed from its own text onto `6855342`, step by step. The tree matched the tests-only and task commits byte for byte.
- After every task the five checks passed: 972 tests after Task 1, 982 after Task 2 and 996 after Task 3 (from 971), apart from `hennery-host`'s runtime tests while this machine's DNS was down; they failed on `main` too, and passed once it was back.
- Every side-effect line and every guard was revert-probed (the lists in each task's Step 5).

## Execution status (2026-10-02)

**Executed** on `main` at `0955dce` (10a merged). As with 10a, the code was built first and reviewed. It was then cut into the three tasks' commits (tests first, then code), and the plan was replayed from its text until the trees matched.

| Area | As built | Why |
|---|---|---|
| The security review (opus, on the maintainer's behalf) | Approved after A1–A5: a resend notifies only once reconciled and only what still holds; a withdrawn-and-asked-again question is quiet; a per-tag coalescing queue; the session read in the fact's transaction; the directory name on one line; named caps. | A resent backlog could have pushed a question already withdrawn. An agent could have flooded urgent pushes on its own pace. One session could have filled the queue for the rest. |
| Its re-confirmation | "Confirmed", with A6 taken: per session, the latest blocking question and the latest notifying turn end are deferred, and an edge that notifies nothing replaces nothing. N1 and N2 are recorded. A connection that drops mid-resend losing its deferred edges is accepted. | A later quiet edge would otherwise hide a "finished". |
| The task review (opus) | Task 1: `question_title` moved below `option_ids`, whose doc comment it had split. Task 3: the socket tests poll the queue for the notice they expect, rather than reading it once after the store shows the fact. The reconciliation test ends on a notifying fact, so a late notice would show. | A notice is queued after its fact's commit, so a single read raced it. |
| The whole-branch review (opus): approve with fixes | `Deferred::keep` names every edge, so a new one does not compile without a slot. `Notice::url` is a same-origin path. The extract's boxing note is kept out of the published schema. The queue's lock survives poisoning. Two tests were added for claims decisions 1 and 2 made (a question with no turn id that blocks; a close mid-turn), and the dates, counts and one probe row without a probe were corrected. | Hand-off text and docs had drifted after the amendments. |
| The probes | Two passed at first and were fixed by better tests. Blanking the title on `pending_opened` broke no test: the two `host_session.rs` tests that run the real host now check the title. Notifying a resend at once was masked by the coalescing queue: the resend test now asserts nothing is queued before `resend_complete`. | A guard no test fails without is unverified. |

Checks:
- After each task the five checks passed: TASKCOUNTS (from 971).
- The PROBES revert-probes each failed their test.
- The five socket tests passed in 3 rounds of 4 parallel copies.
- The run was macOS only, so ubuntu CI is the Linux check.


## Scope

**3 tasks:**
1. The question's title on the wire, from the host.
2. The store's push edges.
3. The notice, its queue, and the triggers.

**Out:**
- delivery: encryption, the egress client, retries, 404/410, the hat's policy applied, `last_success_at` (10b-ii);
- `agent_failure`. No host fills that extract yet: it needs the Codex profile's `sessionFailure` capability (ACP core §6, profiles, not built). 10b-ii, or the profiles plan if it lands first, adds the extract and its edge;
- the gateway's `Notifier` wiring (plan 8): it calls `Push::notify` too.

## Decisions this plan makes where the spec is silent

The security review of 2026-10-02 (opus, on the maintainer's behalf) approved after amendments A1–A5. Its scoped re-confirmation said "confirmed" and added A6. All six are in decisions 1, 2, 4, 5 and 6.

1. **Edges are read in the fact's own transaction, from what it changed.**
   - `Blocked`: the `UPDATE … SET activity = 'blocked' WHERE activity = 'running'` changed a row. A second question finds the turn already blocked and crosses nothing.
   - `TurnEnded`: the `UPDATE` that ends the open turn changed a row. A duplicate, a late `turn_ended` for an ended turn, and one for another turn change none.
   - A duplicate seq and a fact stored but not applied cross nothing: their `created` events are cleared, and so is their edge.
   - The activity decides `Blocked`, not the turn id: a question with no turn id that blocks a running turn is `Blocked` (the task review).
   - **A question asked again after the agent withdrew one in the same turn crosses nothing** (the review's A2), though it blocks the turn. Otherwise an agent could ask and withdraw in a loop, each ask an urgent push, with only the agent pacing them. Answered and asked again is the operator's pace, and still notifies. The invariant: every edge the agent paces is bounded by something the operator paces. A turn's end is bounded by the owner's prompts.
   - The edge is returned with the events (`Ingested`), never guessed from the event kinds afterwards. It carries the session as the fact left it: its hat, title and cwd are read in the same transaction (the review's A3). So a re-assignment or rename landing right after cannot change which hat's policy applies.
2. **Only the host-ingest path notifies, and a resend only once reconciled.** `ws.rs` calls `notify` after the commit and after the events are published. Reconciliation, the presumed park, a revoke and the API's own writes write no host fact, so they cross no edge (ACP core §10, P-25). A park or close that ends a turn (`turn_ended_synthesized`) is not a `turn_ended` either. Pinned in the store and through a host socket.
   - **Facts a host resends after a reconnect** (the review's A1), before its `resend_complete`, are real facts the collector may not have seen. They do notify, but only once reconciliation is done, and only if they still hold.
     - Per session, the connection keeps the latest question that blocked a turn and the latest turn end that notifies. An edge that notifies nothing is not kept, so it cannot hide one that does (the re-confirmation's A6): a backlog that finishes, then asks outside a turn, still gives its "finished".
     - After reconciliation and the answer queue, it notifies the question if it is still open and its turn still blocked (`still_blocked_on`), else the turn's end.
     - Example: a backlog that asks, withdraws and then finishes gives one "finished", never a "needs your answer" for a question already gone.
     - A connection that drops before its `resend_complete` loses its deferred edges: the next connection's resend is all duplicates. Accepted by the re-confirmation. These notices are at most once by design; making them durable would need a persisted outbox of notices. The cockpit still shows the session blocked or ended.
3. **Which edges notify, decided in one place** (`notice_for`):
   - `Blocked`: "needs your answer", `High`, with the question's title as the `detail`;
   - `TurnEnded(completed)`: "finished"; `TurnEnded(failed)`: "failed";
   - `TurnEnded(cancelled)` and `TurnEnded(interrupted)`: none. The operator cancelled; an interruption is the host's restart or a close, which the session itself shows;
   - `QuestionOutsideTurn`: none, for now. **The maintainer's open question** (below). If it ever notifies, it needs a limit in time per session: nothing the operator does paces it (the review's O1). The withdrawal check compares turn ids, and SQL's `NULL = NULL` is never true, so a `Blocked` with no turn id (a stale `running`) is not rate-limited. The outside-turn decision should settle that too (the re-confirmation's N1).
4. **The notice:**
   - The title is the session's, else its project directory's name, put on one line as a title is (the review's A4: a directory name can hold control or bidi characters), else "Session".
   - The generic title is "Session needs your answer", "Session finished" or "Session failed": nothing of the session's own.
   - `url` is `/sessions/<id>`; `tag` is the session id, so a newer notice replaces the older on a device.
   - `hat_id` is the session's, whose policy delivery applies.
   - The policy's meaning is fixed in `Notice`'s doc: muted drops it; `generic_title` takes the generic title and drops the body; `details` puts `detail` in place of the body. 10b-ii implements it.
   - `detail` is agent text. A tool call's title can be a command line with a secret in it. Settings must say so next to `details` (the review's O2, for plan 4).
5. **The queue never blocks ingest, and one session cannot fill it** (the review's A2).
   - It holds at most 256 tags. Each tag holds only its latest notice, in the tag's first place; the tag is the session id, and a device shows one notification per tag anyway.
   - A new tag when 256 wait is dropped and counted (`Notices::dropped`), with a warning at the first drop and every power of two after, naming no notice's text.
   - With no reader (`detached`, or `Notices` dropped) every notice is dropped quietly.
   - `notify` takes a lock for a few map operations and never awaits. A push is not state: losing one never stops a fact being acked.
6. **The question's title** is the host's to read, as the option ids are: the collector never parses a question's payload (ACP core §3.2).
   - The host takes `toolCall.title` for a permission, `message` for an elicitation, cut to 200 characters; blank or absent is `None`.
   - The collector puts it on one line with the session title's own rule and constants (`one_line`, `TITLE_MAX_CHARS` and `TITLE_MAX_JSON_BYTES`: 120 characters, 160 JSON bytes; the review's A5).
   - It reaches a device only under a hat with `details`, inside the payload encrypted to the browser (RFC 8291).
7. **`Indexed::pending` is boxed.** The title made `HostFrame::Session` 224 bytes larger than the next variant, and clippy's `large_enum_variant` refused it. Boxing the extract, which is on one fact in many, puts it back under the limit. The JSON is unchanged: serde, schemars and ts-rs read `Box<T>` as `T`.

### Open product question for the maintainer

A question the agent asks **outside a turn** leaves `activity` alone (ACP core §4.2; umbrella §5), so the `blocked` edge never fires for it. Should it get a push trigger of its own, or none?
- The store reports it as its own edge, `PushEdge::QuestionOutsideTurn`, with its title.
- `notice_for` maps it to `None`, and `a_question_outside_a_turn_does_not_notify_yet` pins that.
- Answering it changes that arm and that test, and `Deferred::keep` in `ws.rs`, whose match names every edge so that a new one does not compile until it has a slot.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task the five checks pass (fmt, both clippy runs, the workspace tests, `gen --check`).
- **No new crates.** **Wire types change in Task 1**; regenerate there.
- The store's new statements name the owner, as all of `store.rs`'s do; the owner audit reads them.
- **No Linux-only code.** No test waits on timing: the end-to-end tests poll for the state they need.
- Nothing is sent anywhere: the collector's queue has no reader yet.
- Commits: Conventional Commits, gmail identity, unsigned.

## Review Focus

1. **A push on recovery** (P-25), and on a resend.
   - Expected: none for a reconciliation, a park or close that ends the turn, or a late or duplicate `turn_ended`. A resent backlog notifies after reconciliation, and only what still holds.
   - Tests:
     - Task 2: `a_turn_ends_once`, `a_park_mid_turn_ends_it_without_an_edge`, `reconciliation_and_the_resend_after_it_cross_nothing`;
     - Task 3: `a_turn_ended_by_reconciliation_queues_no_notice`, `a_resent_backlog_notifies_only_what_still_holds`, `a_resent_question_still_open_notifies_after_reconciliation`.
2. **A push per question, and an agent's flood.**
   - Expected: only the question that blocks the turn notifies. One asked again after a withdrawal does not. A session's burst is one waiting notice.
   - Tests:
     - Task 2: `the_first_question_of_a_turn_blocks_it_and_a_second_does_not`, `a_question_asked_again_after_a_withdrawal_crosses_nothing`;
     - Task 3: `a_blocked_turn_and_its_end_each_queue_one_notice`, `a_question_asked_again_after_a_withdrawal_does_not_notify`, and the unit tests `a_tag_waiting_keeps_its_place_and_takes_the_latest_notice` and `a_full_queue_drops_new_tags_and_counts_them`.
3. **What a notice says.**
   - Expected: the session's title or its directory, the agent's question title only as `detail`, no prompt text or transcript.
   - Tests: Task 2 `an_edge_carries_its_session_as_the_fact_left_it`; Task 3 `only_blocked_finished_and_failed_notify`, `the_title_is_the_sessions_else_its_directorys`, `a_directorys_name_is_put_on_one_line`.
4. **The outside-turn question.**
   - Expected: an edge of its own, no notice yet.
   - Tests: Task 2 `a_question_outside_a_turn_is_an_edge_of_its_own`; Task 3 `a_question_outside_a_turn_does_not_notify_yet`.
5. **Ingest never waits on push.**
   - Expected: `notify` never awaits; a full queue drops and counts.
   - Read: `Push::notify`. Tests: `a_full_queue_drops_new_tags_and_counts_them`, `without_a_reader_every_notice_is_dropped`, `a_reader_waits_for_the_next_notice`.

## File structure

| File | Task | Change |
|---|---|---|
| `crates/hennery-proto/src/frames.rs` | 1 | `PendingExtract::title`; `Indexed::pending` boxed |
| `crates/hennery-host/src/session.rs` | 1 | `question_title`, filled on `pending_opened` |
| `crates/hennery-proto/tests/frames.rs`, `crates/hennery-sessions/tests/store.rs`, `crates/hennery-testkit/tests/host_session.rs`, `reconcile.rs` | 1 | the extract's literals; the real host's titles in `host_session.rs` |
| `schema/`, `web/src/generated/` | 1 | regenerated |
| `crates/hennery-sessions/src/store.rs` | 2 | `Ingested`, `Edge`, `EdgeSession`, `PushEdge`, `ingest_fact`, `still_blocked_on`; `one_line` crate-visible |
| `crates/hennery-sessions/tests/push_edges.rs` | 2, 3 | new |
| `crates/hennery-kernel/src/push.rs` | 3 | `Notice`, `Urgency`, `Push`, `Notices` |
| `crates/hennery-sessions/src/notify.rs` | 3 | new: `notice_for` |
| `crates/hennery-sessions/src/lib.rs`, `ws.rs` | 3 | `AppState::push`; the trigger after a host fact's commit |
| `crates/hennery-testkit/tests/reconcile.rs` | 3 | the collector's notices; five tests |

**Reading the steps.** Each block is one of:
- "Create `path`:" (a new file);
- "In `path`, replace:" with the exact text it replaces, which occurs once, then "with:".

Apply them in order.

---

### Task 1: The question's title on the wire

- [ ] **Step 1: Write the host's test**

In `crates/hennery-host/src/session.rs`, replace:

  ```rust

      /// A boolean would be discarded by the adapter's schema validator,
  ```

with:

  ```rust

      /// Plan 10b: what a question is about, for a push under `details`.
      #[test]
      fn a_questions_title_is_its_tool_call_title_or_its_message() {
          let permission = serde_json::json!({"toolCall": {"toolCallId": "c1", "title": "Run cargo test"}});
          assert_eq!(
              question_title(PendingKind::Permission, &permission).as_deref(),
              Some("Run cargo test")
          );
          let elicitation = serde_json::json!({"message": "Which branch?", "requestedSchema": {}});
          assert_eq!(
              question_title(PendingKind::Elicitation, &elicitation).as_deref(),
              Some("Which branch?")
          );
          // None where the agent gave none, or gave only blanks.
          assert_eq!(
              question_title(PendingKind::Permission, &serde_json::json!({"toolCall": {}})),
              None
          );
          assert_eq!(
              question_title(PendingKind::Elicitation, &serde_json::json!({"message": "  "})),
              None
          );
          assert_eq!(
              question_title(PendingKind::Elicitation, &serde_json::json!({"message": 7})),
              None
          );
          // Cut to MAX_QUESTION_TITLE characters, on a character boundary.
          let long = serde_json::json!({"message": "\u{e9}".repeat(MAX_QUESTION_TITLE + 5)});
          let cut = question_title(PendingKind::Elicitation, &long).unwrap();
          assert_eq!(cut.chars().count(), MAX_QUESTION_TITLE);
      }

      /// A boolean would be discarded by the adapter's schema validator,
  ```

- [ ] **Step 2: Run it, and see it fail**

  `nix develop -c cargo test -p hennery-host --lib a_questions_title` fails to compile: there is no `question_title`.

- [ ] **Step 3: Write the title, and box the extract**

In `crates/hennery-host/src/session.rs`, replace:

  ```rust
              .collect(),
      )
  }

  ```

with:

  ```rust
              .collect(),
      )
  }

  /// The most a question's title carries to the collector, in characters.
  const MAX_QUESTION_TITLE: usize = 200;

  /// What a question is about, as the agent put it (plan 10b): a permission's
  /// tool call title, an elicitation's message, cut to
  /// `MAX_QUESTION_TITLE` characters. Read from the raw request, as its
  /// option ids are; the collector puts it on one line.
  fn question_title(kind: PendingKind, params: &Value) -> Option<String> {
      let title = match kind {
          PendingKind::Permission => params.pointer("/toolCall/title"),
          PendingKind::Elicitation => params.get("message"),
      }?
      .as_str()?;
      let title: String = title.chars().take(MAX_QUESTION_TITLE).collect();
      (!title.trim().is_empty()).then_some(title)
  }

  ```

In `crates/hennery-host/src/session.rs`, replace:

  ```rust
          };
          self.emit(SessionBody::PendingOpened {
  ```

with:

  ```rust
          };
          let title = question_title(kind, &params);
          self.emit(SessionBody::PendingOpened {
  ```

In `crates/hennery-host/src/session.rs`, replace:

  ```rust
                  pending: Some(PendingExtract {
                      id: pending_id.clone(),
                      kind,
                      option_ids,
                  }),
  ```

with:

  ```rust
                  pending: Some(Box::new(PendingExtract {
                      id: pending_id.clone(),
                      kind,
                      option_ids,
                      title,
                  })),
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      pub option_ids: Option<Vec<String>>,
  }
  ```

with:

  ```rust
      pub option_ids: Option<Vec<String>>,
      /// What the question is about, as the agent put it: a permission's tool
      /// call title, an elicitation's message (plan 10b). A push shows it
      /// only under a hat with `details` (kernel spec §6).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub title: Option<String>,
  }
  ```

In `crates/hennery-proto/src/frames.rs`, replace:

  ```rust
      /// On `pending_opened`: the request's id, kind and option ids.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub pending: Option<PendingExtract>,
  ```

with:

  ```rust
      /// On `pending_opened`: the request's id, kind, option ids and title.
      // Boxed: it is on one fact in many, and inline it made every host frame
      // larger (plan 10b-i decision 7).
      #[serde(default, skip_serializing_if = "Option::is_none")]
      pub pending: Option<Box<PendingExtract>>,
  ```

In `crates/hennery-proto/tests/frames.rs`, replace:

  ```rust
              pending: Some(PendingExtract {
                  id: "p1".into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
              }),
  ```

with:

  ```rust
              pending: Some(Box::new(PendingExtract {
                  id: "p1".into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
                  title: None,
              })),
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              turn_id: Some("t1".into()),
              pending: Some(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
              }),
              ..Indexed::default()
  ```

with:

  ```rust
              turn_id: Some("t1".into()),
              pending: Some(Box::new(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
                  title: None,
              })),
              ..Indexed::default()
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              pending: Some(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Elicitation,
                  option_ids: None,
              }),
  ```

with:

  ```rust
              pending: Some(Box::new(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Elicitation,
                  option_ids: None,
                  title: None,
              })),
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              pending: Some(PendingExtract {
                  id: "p1".into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec![]),
              }),
  ```

with:

  ```rust
              pending: Some(Box::new(PendingExtract {
                  id: "p1".into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec![]),
                  title: None,
              })),
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              pending: Some(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
              }),
  ```

with:

  ```rust
              pending: Some(Box::new(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
                  title: None,
              })),
  ```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

  ```rust
                  indexed.pending.clone().unwrap(),
  ```

with:

  ```rust
                  *indexed.pending.clone().unwrap(),
  ```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

  ```rust
      assert_eq!(payload["toolCall"]["toolCallId"], "call-1");

  ```

with:

  ```rust
      assert_eq!(payload["toolCall"]["toolCallId"], "call-1");
      // Its tool call's title, for a push with details (plan 10b).
      assert_eq!(extract.title.as_deref(), Some("Write notes.txt"));

  ```

In `crates/hennery-testkit/tests/host_session.rs`, replace:

  ```rust
      assert_eq!((extract.kind, extract.option_ids), (PendingKind::Elicitation, None));
  ```

with:

  ```rust
      assert_eq!(
          (extract.kind, extract.option_ids, extract.title.as_deref()),
          (PendingKind::Elicitation, None, Some("What should the file be called?"))
      );
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              pending: Some(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
              }),
  ```

with:

  ```rust
              pending: Some(Box::new(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into(), "reject".into()]),
                  title: None,
              })),
  ```

  Then regenerate the protocol files: `nix develop -c cargo run -p hennery-proto --bin gen`.

- [ ] **Step 4: Run the test, and the five checks**

- [ ] **Step 5: Revert-probe**

  | Change | Fails |
  |---|---|
  | `toolCall/name` for `toolCall/title` | `a_questions_title_is_its_tool_call_title_or_its_message` |
  | no cut to `MAX_QUESTION_TITLE` | the same |
  | a blank title kept | the same |
  | `title: None` on `pending_opened` | `a_permission_request_waits_for_the_operator_and_the_answer_reaches_the_agent`, `an_elicitation_reaches_the_operator_and_the_form_content_reaches_the_agent` |

- [ ] **Step 6: Commit**: `test(host): a question's title, from its tool call or its message`, then `feat(host): pending_opened carries the question's title, for a push with details`.

### Task 2: The store's push edges

- [ ] **Step 1: Write the edges' tests**

Create `crates/hennery-sessions/tests/push_edges.rs`:

  ```rust
  //! Push triggers (ACP core §10; plan 10b): the edges a host fact crosses,
  //! read by the store in the fact's own transaction. Edge-triggered only: a duplicate, a fact not applied, an already
  //! ended turn and a second question cross nothing.

  use hennery_proto::frames::{Indexed, PendingExtract, PendingKind, SessionBody, TurnOutcome};
  use hennery_sessions::store::{EdgeSession, PushEdge, Store};
  use serde_json::json;

  /// `s1`, started, with turn `t1` open and running (facts 1 and 2).
  fn running(store: &Store) {
      store
          .create_session("s1", "h1", "fake", "/home/me/project", "hat-1", None)
          .unwrap();
      store
          .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
          .unwrap();
      assert!(
          store
              .open_turn("s1", "t1", &[json!({"type": "text", "text": "hi"})])
              .unwrap()
      );
      store
          .ingest(
              "s1",
              2,
              &SessionBody::TurnStarted {
                  request_id: "req-t1".into(),
                  turn_id: "t1".into(),
              },
          )
          .unwrap();
  }

  fn question(pending_id: &str, turn: Option<&str>, title: Option<&str>) -> SessionBody {
      SessionBody::PendingOpened {
          pending_id: pending_id.into(),
          indexed: Indexed {
              turn_id: turn.map(str::to_string),
              pending: Some(Box::new(PendingExtract {
                  id: pending_id.into(),
                  kind: PendingKind::Permission,
                  option_ids: Some(vec!["allow".into()]),
                  title: title.map(str::to_string),
              })),
              ..Indexed::default()
          },
          payload: json!({"toolCall": {"toolCallId": "call-1"}}),
      }
  }

  fn ended(turn: &str, outcome: TurnOutcome) -> SessionBody {
      SessionBody::TurnEnded {
          turn_id: turn.into(),
          outcome,
          stop_reason: None,
          error: None,
      }
  }

  fn edge(store: &Store, seq: u64, body: &SessionBody) -> Option<PushEdge> {
      store.ingest_fact("s1", seq, body).unwrap().edge.map(|e| e.kind)
  }

  #[test]
  fn the_first_question_of_a_turn_blocks_it_and_a_second_does_not() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      assert_eq!(
          edge(&store, 3, &question("p1", Some("t1"), Some("Run\n  cargo test"))),
          Some(PushEdge::Blocked {
              pending_id: "p1".into(),
              title: Some("Run cargo test".into()),
          })
      );
      assert_eq!(edge(&store, 4, &question("p2", Some("t1"), None)), None);
      // The same fact again is a duplicate: nothing.
      assert_eq!(
          edge(&store, 3, &question("p1", Some("t1"), Some("Run cargo test"))),
          None
      );
  }

  #[test]
  fn a_question_outside_a_turn_is_an_edge_of_its_own() {
      let store = Store::open_in_memory().unwrap();
      store
          .create_session("s1", "h1", "fake", "/home/me/project", "hat-1", None)
          .unwrap();
      store
          .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
          .unwrap();
      assert_eq!(
          edge(&store, 2, &question("p1", None, Some("Which branch?"))),
          Some(PushEdge::QuestionOutsideTurn {
              pending_id: "p1".into(),
              title: Some("Which branch?".into()),
          })
      );
      // It left the activity alone.
      assert_eq!(store.session("s1").unwrap().unwrap().activity.as_deref(), Some("idle"));
  }

  #[test]
  fn a_turn_ends_once() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      assert_eq!(
          edge(&store, 3, &ended("t1", TurnOutcome::Completed)),
          Some(PushEdge::TurnEnded(TurnOutcome::Completed))
      );
      // A late duplicate for the ended turn is stored, not applied: nothing.
      assert_eq!(edge(&store, 4, &ended("t1", TurnOutcome::Completed)), None);
      // A turn that is not the open one: nothing.
      assert_eq!(edge(&store, 5, &ended("t9", TurnOutcome::Failed)), None);
  }

  #[test]
  fn a_park_mid_turn_ends_it_without_an_edge() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      let parked = store
          .ingest_fact(
              "s1",
              3,
              &SessionBody::SessionParked {
                  reason: hennery_proto::frames::ParkReason::Idle,
              },
          )
          .unwrap();
      // The turn is ended for it (`turn_ended_synthesized`), which is not a
      // host's `turn_ended`: no push (ACP core §10, P-25).
      assert!(parked.events.iter().any(|e| e.kind == "turn_ended_synthesized"));
      assert_eq!(parked.edge, None);
  }

  #[test]
  fn a_question_for_a_closed_session_is_not_applied_and_crosses_nothing() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      store.ingest("s1", 3, &SessionBody::SessionClosed).unwrap();
      assert_eq!(edge(&store, 4, &question("p1", Some("t1"), None)), None);
  }

  #[test]
  fn reconciliation_and_the_resend_after_it_cross_nothing() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      // The host restarted: reconciliation ends the turn, writing no fact.
      let reconciled = store.reconcile_host("h1", &[]).unwrap();
      assert!(reconciled.events.iter().any(|e| e.kind == "turn_ended_synthesized"));
      // Its outbox resent after: the facts it already had are duplicates.
      assert_eq!(
          edge(
              &store,
              2,
              &SessionBody::TurnStarted {
                  request_id: "req-t1".into(),
                  turn_id: "t1".into()
              }
          ),
          None
      );
      assert_eq!(edge(&store, 3, &ended("t1", TurnOutcome::Completed)), None);
  }

  /// 10b-i's review, A2: asked, withdrawn by the agent, asked again in the
  /// same turn: the second ask blocks the turn but notifies nothing.
  #[test]
  fn a_question_asked_again_after_a_withdrawal_crosses_nothing() {
      use hennery_proto::frames::{PendingReason, PendingResolution};
      let store = Store::open_in_memory().unwrap();
      running(&store);
      assert!(matches!(
          edge(&store, 3, &question("p1", Some("t1"), None)),
          Some(PushEdge::Blocked { .. })
      ));
      let withdrawn = SessionBody::PendingResolved {
          pending_id: "p1".into(),
          resolution: PendingResolution::Cancelled,
          reason: Some(PendingReason::AgentWithdrew),
      };
      store.ingest("s1", 4, &withdrawn).unwrap();
      assert_eq!(edge(&store, 5, &question("p2", Some("t1"), None)), None);
      assert_eq!(
          store.session("s1").unwrap().unwrap().activity.as_deref(),
          Some("blocked")
      );
  }

  /// 10b-i's review, A3: the edge carries the session as its fact left it.
  #[test]
  fn an_edge_carries_its_session_as_the_fact_left_it() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      let edge = store
          .ingest_fact("s1", 3, &question("p1", Some("t1"), None))
          .unwrap()
          .edge
          .unwrap();
      assert_eq!(
          edge.session,
          EdgeSession {
              id: "s1".into(),
              hat_id: "hat-1".into(),
              title: None,
              cwd: "/home/me/project".into(),
          }
      );
  }

  /// Decision 1: the activity decides, not the turn id. A question with no
  /// turn id that blocks a running turn is `Blocked`.
  #[test]
  fn a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      assert!(matches!(
          edge(&store, 3, &question("p1", None, None)),
          Some(PushEdge::Blocked { .. })
      ));
  }

  #[test]
  fn a_close_mid_turn_ends_it_without_an_edge() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      let closed = store.ingest_fact("s1", 3, &SessionBody::SessionClosed).unwrap();
      assert!(closed.events.iter().any(|e| e.kind == "turn_ended_synthesized"));
      assert_eq!(closed.edge, None);
  }
  ```

- [ ] **Step 2: Run them, and see them fail**

  `nix develop -c cargo test -p hennery-sessions --test push_edges` fails to compile: there is no `ingest_fact` or `PushEdge`.

- [ ] **Step 3: Read the edge in the fact's transaction**

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  ",
  ];

  #[derive(Debug, Clone, PartialEq)]
  ```

with:

  ```rust
  ",
  ];

  /// What `Store::ingest_fact` did with one fact.
  #[derive(Debug, Clone, PartialEq)]
  pub struct Ingested {
      /// The events it created, in order: none for a duplicate or a fact not
      /// applied.
      pub events: Vec<EventDto>,
      /// The push edge it crossed, if any.
      pub edge: Option<Edge>,
  }

  /// A push edge, with the session as the fact left it: read in the fact's
  /// transaction, so a re-assignment or rename after it cannot change which
  /// hat's policy applies (10b-i's review, A3).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Edge {
      pub kind: PushEdge,
      pub session: EdgeSession,
  }

  /// What a notice needs of its session.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct EdgeSession {
      pub id: String,
      pub hat_id: String,
      /// On one line and capped, as stored.
      pub title: Option<String>,
      pub cwd: String,
  }

  /// The edge's session from a row read later: for tests, and for any caller
  /// that holds a row rather than an edge.
  impl From<&SessionRow> for EdgeSession {
      fn from(row: &SessionRow) -> Self {
          Self {
              id: row.id.clone(),
              hat_id: row.hat_id.clone(),
              title: row.title.clone(),
              cwd: row.cwd.clone(),
          }
      }
  }

  /// A change a host fact made that may notify the owner (ACP core §10).
  /// Only an applied fact, ingested from its host, crosses one: recovery and
  /// reconciliation write no facts, and never push. Which edges notify is
  /// decided in one place (`notify::notice_for`).
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum PushEdge {
      /// The session's activity went from `running` to `blocked`: the first
      /// open question of the turn. `title` is the question's, from the host
      /// (`PendingExtract::title`), on one line.
      Blocked { pending_id: String, title: Option<String> },
      /// A question the agent asked outside a turn: it leaves the activity
      /// alone (ACP core §4.2), so it is not `Blocked`.
      QuestionOutsideTurn { pending_id: String, title: Option<String> },
      /// The open turn ended with this outcome (a real `turn_ended`; a
      /// synthesised one is never a fact).
      TurnEnded(TurnOutcome),
  }

  #[derive(Debug, Clone, PartialEq)]
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  fn one_line(raw: &str, max_chars: usize, max_json_bytes: usize) -> Option<String> {
  ```

with:

  ```rust
  pub(crate) fn one_line(raw: &str, max_chars: usize, max_json_bytes: usize) -> Option<String> {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// Ingest one sequenced host frame. Idempotent on (session_id, seq): a
      /// duplicate with the same body is discarded; one with a different body
      /// is kept as a `conflict` event (ACP core §3.6). Returns the events it
      /// created, in order. A frame for a session that is not the owner's
      /// fails, and nothing is written.
      pub fn ingest(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Vec<EventDto>> {
  ```

with:

  ```rust
      /// Ingest one sequenced host frame (`ingest_fact`): the events it
      /// created, in order.
      pub fn ingest(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Vec<EventDto>> {
          Ok(self.ingest_fact(session_id, seq, body)?.events)
      }

      /// Ingest one sequenced host frame. Idempotent on (session_id, seq): a
      /// duplicate with the same body is discarded; one with a different body
      /// is kept as a `conflict` event (ACP core §3.6). Returns the events it
      /// created, in order, and the push edge it crossed, if any (ACP core
      /// §10; plan 10b): read in the same transaction, from what the fact
      /// changed, so a duplicate, a fact stored but not applied, and a fact
      /// for an already ended turn cross none. A frame for a session that is
      /// not the owner's fails, and nothing is written.
      pub fn ingest_fact(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Ingested> {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              return Ok(created);
          }
          let fact_id = tx.last_insert_rowid();
  ```

with:

  ```rust
              return Ok(Ingested {
                  events: created,
                  edge: None,
              });
          }
          let fact_id = tx.last_insert_rowid();
          let mut edge = None;
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  if applied == 0 {
                      created.clear();
                      mark_unapplied(&tx, &self.owner, fact_id)?;
                  }
              }
              SessionBody::SessionParked { reason } => {
  ```

with:

  ```rust
                  if applied == 0 {
                      created.clear();
                      mark_unapplied(&tx, &self.owner, fact_id)?;
                  } else {
                      edge = Some(PushEdge::TurnEnded(*outcome));
                  }
              }
              SessionBody::SessionParked { reason } => {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      tx.execute(
                          "UPDATE sessions SET activity = 'blocked' WHERE id = ?1 AND activity = 'running' AND owner_id = ?2",
                          [session_id, &self.owner],
                      )?;
  ```

with:

  ```rust
                      let blocked = tx.execute(
                          "UPDATE sessions SET activity = 'blocked' WHERE id = ?1 AND activity = 'running' AND owner_id = ?2",
                          [session_id, &self.owner],
                      )?;
                      // Edge-triggered: only the question that blocks the
                      // turn; a second one finds it blocked already. The
                      // activity decides, not the turn id: a question with
                      // none that blocks a running turn is `Blocked`. One that
                      // leaves the activity alone (ACP core §4.2), asked
                      // outside a turn, is an edge of its own.
                      let title = extract
                          .and_then(|e| e.title.as_deref())
                          .and_then(|t| one_line(t, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES));
                      // A question asked again after the agent withdrew one
                      // in the same turn notifies nothing: an agent could
                      // otherwise ask and withdraw in a loop, each one an
                      // urgent push, with no one but it pacing them
                      // (10b-i's review, A2). Answered and asked again is
                      // the operator's pace, and still notifies.
                      let rewithdrawn = blocked > 0
                          && tx.query_row(
                              "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND turn_id = ?2
                                   AND pending_id <> ?3 AND reason = 'agent_withdrew' AND owner_id = ?4)",
                              params![session_id, indexed.turn_id, pending_id, self.owner],
                              |r| r.get::<_, bool>(0),
                          )?;
                      edge = if rewithdrawn {
                          None
                      } else if blocked > 0 {
                          Some(PushEdge::Blocked {
                              pending_id: pending_id.clone(),
                              title,
                          })
                      } else if indexed.turn_id.is_none() {
                          Some(PushEdge::QuestionOutsideTurn {
                              pending_id: pending_id.clone(),
                              title,
                          })
                      } else {
                          None
                      };
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          Ok(created)
  ```

with:

  ```rust
          // The session as this fact left it, for the notice (A3).
          let edge = match edge {
              Some(kind) => Some(Edge {
                  kind,
                  session: tx.query_row(
                      "SELECT id, hat_id, title, cwd FROM sessions WHERE id = ?1 AND owner_id = ?2",
                      [session_id, &self.owner],
                      |r| {
                          Ok(EdgeSession {
                              id: r.get(0)?,
                              hat_id: r.get(1)?,
                              title: r.get(2)?,
                              cwd: r.get(3)?,
                          })
                      },
                  )?,
              }),
              None => None,
          };
          tx.commit()?;
          Ok(Ingested { events: created, edge })
      }

      /// Whether `pending_id` is still open in `session_id`, and the session
      /// still blocked: a `Blocked` edge the host resent before reconnecting
      /// is notified only if it still holds once the resend is complete
      /// (10b-i's review, A1).
      pub fn still_blocked_on(&self, session_id: &str, pending_id: &str) -> Result<bool> {
          Ok(self.conn().query_row(
              "SELECT EXISTS(SELECT 1 FROM pending p JOIN sessions s ON s.id = p.session_id AND s.owner_id = p.owner_id
                   WHERE p.pending_id = ?1 AND p.session_id = ?2 AND p.state = 'open' AND s.activity = 'blocked'
                       AND p.owner_id = ?3)",
              [pending_id, session_id, &self.owner],
              |r| r.get(0),
          )?)
  ```

- [ ] **Step 4: Run the tests (10), and the five checks**

- [ ] **Step 5: Revert-probe**

  | Change | Fails |
  |---|---|
  | `Blocked` whatever the `UPDATE` changed | `the_first_question_of_a_turn_blocks_it_and_a_second_does_not` |
  | no `QuestionOutsideTurn` arm | `a_question_outside_a_turn_is_an_edge_of_its_own` |
  | `TurnEnded` for an end not applied | `a_turn_ends_once` |
  | no `one_line` on the title | the same |
  | `Blocked` only with a turn id | `a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked` |
  | the withdrawal check dropped | `a_question_asked_again_after_a_withdrawal_crosses_nothing` |
  | the edge's session read after the commit, from another row | `an_edge_carries_its_session_as_the_fact_left_it` |

- [ ] **Step 6: Commit**: `test(sessions): the push edges a host fact crosses, and the ones it never does`, then `feat(sessions): the store reports the push edge a fact crosses, read in its transaction`.

### Task 3: The notice, its queue, and the triggers

- [ ] **Step 1: Write the triggers' tests**

In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
  //! read by the store in the fact's own transaction. Edge-triggered only: a duplicate, a fact not applied, an already
  //! ended turn and a second question cross nothing.

  use hennery_proto::frames::{Indexed, PendingExtract, PendingKind, SessionBody, TurnOutcome};
  ```

with:

  ```rust
  //! read by the store in the fact's own transaction, and which of them
  //! notify. Edge-triggered only: a duplicate, a fact not applied, an already
  //! ended turn and a second question cross nothing.

  use hennery_kernel::push::Urgency;
  use hennery_proto::frames::{Indexed, PendingExtract, PendingKind, SessionBody, TurnOutcome};
  use hennery_sessions::notify::notice_for;
  ```

In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
      assert_eq!(closed.edge, None);
  }
  ```

with:

  ```rust
      assert_eq!(closed.edge, None);
  }

  fn session(store: &Store) -> EdgeSession {
      (&store.session("s1").unwrap().unwrap()).into()
  }

  #[test]
  fn only_blocked_finished_and_failed_notify() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      let s = session(&store);
      let blocked = notice_for(
          &PushEdge::Blocked {
              pending_id: "p1".into(),
              title: Some("Run cargo test".into()),
          },
          &s,
      )
      .unwrap();
      assert_eq!(
          (blocked.urgency, blocked.title.as_str(), blocked.body.as_str()),
          (Urgency::High, "project", "needs your answer")
      );
      assert_eq!(blocked.generic_title, "Session needs your answer");
      assert_eq!(blocked.detail.as_deref(), Some("Run cargo test"));
      assert_eq!(
          (blocked.url.as_str(), blocked.tag.as_str(), blocked.hat_id.as_str()),
          ("/sessions/s1", "s1", "hat-1")
      );

      let finished = notice_for(&PushEdge::TurnEnded(TurnOutcome::Completed), &s).unwrap();
      assert_eq!(
          (finished.urgency, finished.body.as_str(), finished.detail),
          (Urgency::Normal, "finished", None)
      );
      let failed = notice_for(&PushEdge::TurnEnded(TurnOutcome::Failed), &s).unwrap();
      assert_eq!(
          (failed.body.as_str(), failed.generic_title.as_str()),
          ("failed", "Session failed")
      );
      for quiet in [TurnOutcome::Cancelled, TurnOutcome::Interrupted] {
          assert_eq!(notice_for(&PushEdge::TurnEnded(quiet), &s), None);
      }
  }

  /// The maintainer's open question (plan 10b): until it is answered, a
  /// question asked outside a turn does not notify. Answering it changes
  /// `notice_for` and this test.
  #[test]
  fn a_question_outside_a_turn_does_not_notify_yet() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      let edge = PushEdge::QuestionOutsideTurn {
          pending_id: "p1".into(),
          title: None,
      };
      assert_eq!(notice_for(&edge, &session(&store)), None);
  }

  #[test]
  fn the_title_is_the_sessions_else_its_directorys() {
      let store = Store::open_in_memory().unwrap();
      running(&store);
      store
          .ingest(
              "s1",
              3,
              &SessionBody::AcpUpdate {
                  indexed: Indexed {
                      title: Some("Fix the flaky test".into()),
                      ..Indexed::default()
                  },
                  payload: json!({"sessionUpdate": "session_info_update"}),
              },
          )
          .unwrap();
      let notice = notice_for(&PushEdge::TurnEnded(TurnOutcome::Completed), &session(&store)).unwrap();
      assert_eq!(notice.title, "Fix the flaky test");
  }

  /// 10b-i's review, A4: a directory's name reaches a lock screen like a
  /// title does, on one line, with no invisible or control characters.
  #[test]
  fn a_directorys_name_is_put_on_one_line() {
      let at = |cwd: &str| EdgeSession {
          id: "s1".into(),
          hat_id: "hat-1".into(),
          title: None,
          cwd: cwd.into(),
      };
      let ended = PushEdge::TurnEnded(TurnOutcome::Completed);
      assert_eq!(
          notice_for(&ended, &at("/home/me/evil\u{202e}txt.exe\nname"))
              .unwrap()
              .title,
          "eviltxt.exe name"
      );
      assert_eq!(notice_for(&ended, &at("/")).unwrap().title, "Session");
  }
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      _dir: tempfile::TempDir,
  }
  ```

with:

  ```rust
      _dir: tempfile::TempDir,
      /// What the push triggers queued (plan 10b), unread by any delivery.
      notices: std::sync::Mutex<hennery_kernel::push::Notices>,
  }
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          tokio::spawn(hennery_sessions::serve(listener, state.clone()));
          Self { addr, state, _dir: dir }
  ```

with:

  ```rust
          let (push, notices) = hennery_kernel::push::Push::new();
          state.push = push;
          tokio::spawn(hennery_sessions::serve(listener, state.clone()));
          Self {
              addr,
              state,
              _dir: dir,
              notices: std::sync::Mutex::new(notices),
          }
      }

      /// Every notice queued since the last call.
      fn notices(&self) -> Vec<hennery_kernel::push::Notice> {
          let mut queue = self.notices.lock().unwrap();
          std::iter::from_fn(|| queue.try_recv()).collect()
      }

      /// The notices queued from now until one has `body`, that one last: a
      /// notice is queued after its fact's commit, so the store showing the
      /// fact does not mean the notice is there yet.
      async fn notices_until(&self, body: &str) -> Vec<hennery_kernel::push::Notice> {
          let mut seen = Vec::new();
          wait_for(body, || {
              seen.extend(self.notices());
              let done = seen.iter().any(|n| n.body == body);
              async move { done.then_some(()) }
          })
          .await;
          seen
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      assert_eq!((row.lifecycle.as_str(), row.hat_id), ("closed", acme.id));
  }
  ```

with:

  ```rust
      assert_eq!((row.lifecycle.as_str(), row.hat_id), ("closed", acme.id));
  }

  // Plan 10b: push triggers, from the host's facts only (ACP core §10).

  /// A question blocks the turn: one urgent notice; a second question, none;
  /// the turn's end: "finished".
  #[tokio::test]
  async fn a_blocked_turn_and_its_end_each_queue_one_notice() {
      use hennery_kernel::push::Urgency;
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let (session, turn) = asking_session(&collector, &mut host).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
      assert_eq!(
          (notices[0].urgency, notices[0].body.as_str(), notices[0].tag.as_str()),
          (Urgency::High, "needs your answer", session.as_str())
      );
      assert_eq!(notices[0].url, format!("/sessions/{session}"));
      host.emit(&session, opened("p2", &turn)).await;
      host.emit(
          &session,
          turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
      )
      .await;
      // One socket's frames are handled in order: by the end's notice, `p2`
      // was handled, and queued none.
      let notices = collector.notices_until("finished").await;
      assert_eq!(
          notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
          ["finished"]
      );
  }

  /// A host that restarted mid-turn: reconciliation ends the turn, and
  /// nothing is pushed for it (P-25). Then a question in another session's
  /// turn, on the same socket: its notice, queued after everything before
  /// it, is the only one.
  #[tokio::test]
  async fn a_turn_ended_by_reconciliation_queues_no_notice() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      started_turn(&collector, &mut host, &session).await;
      host.drop_connection(&collector).await;
      let mut host = ScriptedHost::hello(&collector, vec![], 0).await;
      host.send(&HostFrame::ResendComplete).await;
      wait_for("parked", || async {
          (collector.lifecycle(&session) == "parked").then_some(())
      })
      .await;
      assert!(
          collector
              .event_kinds(&session)
              .contains(&"turn_ended_synthesized".to_string())
      );
      let (other, _) = asking_session(&collector, &mut host).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
      assert_eq!(notices[0].tag, other);
  }

  /// 10b-i's review, A1: facts the host resends after a reconnect notify
  /// only once reconciliation is done, and only what still holds. A
  /// question asked, withdrawn and then the turn finished, all in the
  /// backlog: one "finished", no "needs your answer".
  #[tokio::test]
  async fn a_resent_backlog_notifies_only_what_still_holds() {
      use hennery_proto::frames::{PendingReason, PendingResolution, TurnOutcome};
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let turn = started_turn(&collector, &mut host, &session).await;
      let seq = host.seq;
      host.drop_connection(&collector).await;
      let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
      host.emit(&session, opened("p1", &turn)).await;
      host.emit(
          &session,
          SessionBody::PendingResolved {
              pending_id: "p1".into(),
              resolution: PendingResolution::Cancelled,
              reason: Some(PendingReason::AgentWithdrew),
          },
      )
      .await;
      host.emit(&session, turn_ended(&turn, TurnOutcome::Completed)).await;
      // A6: a later edge that notifies nothing (a question outside any turn)
      // must not hide the "finished".
      host.emit(&session, outside("p2")).await;
      wait_for("the backlog", || async {
          (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
      })
      .await;
      // Ingested, and nothing queued while the resend runs.
      assert!(collector.notices().is_empty());
      host.send(&HostFrame::ResendComplete).await;
      let notices = collector.notices_until("finished").await;
      assert_eq!(
          notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
          ["finished"]
      );
  }

  /// A1, the other way: a question in the backlog still open once the host
  /// is reconciled does ask the owner, then.
  #[tokio::test]
  async fn a_resent_question_still_open_notifies_after_reconciliation() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let turn = started_turn(&collector, &mut host, &session).await;
      let seq = host.seq;
      host.drop_connection(&collector).await;
      let mut host = ScriptedHost::hello(
          &collector,
          vec![AttachedSession {
              session_id: session.clone(),
              last_seq: seq,
              open_turn_id: Some(turn.clone()),
          }],
          seq,
      )
      .await;
      host.emit(&session, opened("p1", &turn)).await;
      wait_for("the question", || async {
          (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
      })
      .await;
      // Ingested, but not notified while the resend runs.
      assert!(collector.notices().is_empty());
      host.send(&HostFrame::ResendComplete).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
  }

  /// 10b-i's review, A2: a question asked again after the agent withdrew one
  /// in the same turn does not notify; it is the agent's pace, not the
  /// owner's.
  #[tokio::test]
  async fn a_question_asked_again_after_a_withdrawal_does_not_notify() {
      use hennery_proto::frames::{PendingReason, PendingResolution};
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let (session, turn) = asking_session(&collector, &mut host).await;
      collector.notices_until("needs your answer").await;
      host.emit(
          &session,
          SessionBody::PendingResolved {
              pending_id: "p1".into(),
              resolution: PendingResolution::Cancelled,
              reason: Some(PendingReason::AgentWithdrew),
          },
      )
      .await;
      host.emit(&session, opened("p2", &turn)).await;
      host.emit(
          &session,
          turn_ended(&turn, hennery_proto::frames::TurnOutcome::Completed),
      )
      .await;
      let notices = collector.notices_until("finished").await;
      assert_eq!(
          notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
          ["finished"]
      );
  }

  /// A question asked outside any turn.
  fn outside(pending_id: &str) -> SessionBody {
      let SessionBody::PendingOpened {
          pending_id,
          mut indexed,
          payload,
      } = opened(pending_id, "unused")
      else {
          unreachable!("`opened` makes a question");
      };
      indexed.turn_id = None;
      SessionBody::PendingOpened {
          pending_id,
          indexed,
          payload,
      }
  }
  ```

- [ ] **Step 2: Run them, and see them fail**

  `nix develop -c cargo test -p hennery-sessions --test push_edges` fails to compile: there is no `notify::notice_for`.

- [ ] **Step 3: Write the notice, the queue and the trigger**

In `crates/hennery-kernel/src/push.rs`, replace:

  ```rust

  #[cfg(test)]
  ```

with:

  ```rust

  /// How soon a push service should deliver a notification (RFC 8030 §5.3):
  /// `high` for "needs your answer", `normal` otherwise (kernel spec §6).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Urgency {
      High,
      Normal,
  }

  /// A notification for the owner's devices, before the hat's policy is
  /// applied (kernel spec §6; plan 10b decision 1). The modules that own a
  /// trigger fill it in (ACP core §10, gateway §7); delivery applies the
  /// policy, so no caller re-implements it:
  /// - a muted hat: nothing is sent;
  /// - `generic_title`: `generic_title` replaces `title`, and `body` is
  ///   dropped;
  /// - `details`: `detail`, when there is one, replaces `body`.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Notice {
      /// The hat whose policy applies; `''` (a session from before hats) has
      /// the default.
      pub hat_id: String,
      pub urgency: Urgency,
      /// The session title, or the connection's label.
      pub title: String,
      /// The title with nothing of the session's own: "Session needs your
      /// answer".
      pub generic_title: String,
      pub body: String,
      /// More than the default shows (the agent's question title), only
      /// under a hat with `details`.
      pub detail: Option<String>,
      /// The same-origin path the notification opens: `/sessions/<id>`, or
      /// `/mcp` for the gateway's. Never a URL.
      pub url: String,
      /// One notification per tag on a device: a newer one replaces it.
      pub tag: String,
  }

  /// The tags waiting to be delivered, at most (plan 10b-i decision 5). A
  /// tag holds one notice, its latest, so one session cannot fill the queue
  /// for the others (10b-i's review, A2).
  pub const NOTICE_QUEUE: usize = 256;

  #[derive(Default)]
  struct Queue {
      /// Tags in the order they were first queued.
      order: std::collections::VecDeque<String>,
      /// Each queued tag's latest notice.
      latest: std::collections::HashMap<String, Notice>,
      /// Notices dropped because `NOTICE_QUEUE` tags were waiting.
      dropped: u64,
      /// Whether a reader is there; without one, every notice is dropped.
      read: bool,
  }

  struct Shared {
      queue: std::sync::Mutex<Queue>,
      ready: tokio::sync::Notify,
  }

  /// Where notices go to be delivered (kernel spec §6): a queue that
  /// delivery (plan 10b-ii) drains. Cheap to clone; `notify` never waits and
  /// never fails, so a trigger can call it from any thread, async or not.
  #[derive(Clone)]
  pub struct Push {
      shared: std::sync::Arc<Shared>,
  }

  /// The queue's reading end, for delivery. Dropping it makes `notify` drop
  /// everything.
  pub struct Notices {
      shared: std::sync::Arc<Shared>,
  }

  impl Push {
      /// A queue of `NOTICE_QUEUE` tags and its reading end.
      pub fn new() -> (Push, Notices) {
          let shared = std::sync::Arc::new(Shared {
              queue: std::sync::Mutex::new(Queue {
                  read: true,
                  ..Queue::default()
              }),
              ready: tokio::sync::Notify::new(),
          });
          (Push { shared: shared.clone() }, Notices { shared })
      }

      /// A queue nobody reads: every notice is dropped. What a collector has
      /// until delivery runs, and tests that do not look.
      pub fn detached() -> Push {
          Push::new().0
      }

      /// Queue `notice` for delivery. Never waits. A notice for a tag already
      /// waiting replaces it there, keeping its place: a device shows one
      /// notification per tag anyway. A new tag when `NOTICE_QUEUE` are
      /// waiting is dropped and counted, with a warning at the first drop and
      /// every power of two after. The log names no notice's text.
      pub fn notify(&self, notice: Notice) {
          let mut queue = self
              .shared
              .queue
              .lock()
              .unwrap_or_else(std::sync::PoisonError::into_inner);
          if !queue.read {
              return;
          }
          if let Some(waiting) = queue.latest.get_mut(&notice.tag) {
              *waiting = notice;
          } else if queue.order.len() >= NOTICE_QUEUE {
              queue.dropped += 1;
              if queue.dropped.is_power_of_two() {
                  tracing::warn!(dropped = queue.dropped, "push queue full; notifications dropped");
              }
              return;
          } else {
              queue.order.push_back(notice.tag.clone());
              queue.latest.insert(notice.tag.clone(), notice);
          }
          drop(queue);
          self.shared.ready.notify_one();
      }
  }

  impl Notices {
      /// The next notice, oldest tag first; waits for one.
      pub async fn recv(&mut self) -> Notice {
          let shared = self.shared.clone();
          loop {
              let ready = shared.ready.notified();
              if let Some(notice) = self.try_recv() {
                  return notice;
              }
              ready.await;
          }
      }

      /// The next notice, if one is waiting.
      pub fn try_recv(&mut self) -> Option<Notice> {
          let mut queue = self
              .shared
              .queue
              .lock()
              .unwrap_or_else(std::sync::PoisonError::into_inner);
          let tag = queue.order.pop_front()?;
          queue.latest.remove(&tag)
      }

      /// Notices dropped so far because the queue was full.
      pub fn dropped(&self) -> u64 {
          self.shared
              .queue
              .lock()
              .unwrap_or_else(std::sync::PoisonError::into_inner)
              .dropped
      }
  }

  impl Drop for Notices {
      fn drop(&mut self) {
          let mut queue = self
              .shared
              .queue
              .lock()
              .unwrap_or_else(std::sync::PoisonError::into_inner);
          queue.read = false;
          queue.order.clear();
          queue.latest.clear();
      }
  }

  #[cfg(test)]
  ```

In `crates/hennery-kernel/src/push.rs`, replace:

  ```rust
      use serde_json::{Value, json};

  ```

with:

  ```rust
      use serde_json::{Value, json};

      fn notice(tag: &str, body: &str) -> Notice {
          Notice {
              hat_id: "hat-1".into(),
              urgency: Urgency::Normal,
              title: "t".into(),
              generic_title: "g".into(),
              body: body.into(),
              detail: None,
              url: format!("/sessions/{tag}"),
              tag: tag.into(),
          }
      }

      /// 10b-i's review, A2: a tag holds its latest notice, in its first
      /// place, so one session's burst is one notice.
      #[test]
      fn a_tag_waiting_keeps_its_place_and_takes_the_latest_notice() {
          let (push, mut notices) = Push::new();
          push.notify(notice("s1", "needs your answer"));
          push.notify(notice("s2", "finished"));
          push.notify(notice("s1", "failed"));
          assert_eq!(
              notices.try_recv().map(|n| (n.tag, n.body)),
              Some(("s1".into(), "failed".into()))
          );
          assert_eq!(notices.try_recv().map(|n| n.tag), Some("s2".into()));
          assert_eq!(notices.try_recv(), None);
      }

      #[test]
      fn a_full_queue_drops_new_tags_and_counts_them() {
          let (push, mut notices) = Push::new();
          for n in 0..NOTICE_QUEUE + 3 {
              push.notify(notice(&format!("s{n}"), "finished"));
          }
          assert_eq!(notices.dropped(), 3);
          // A waiting tag is still replaced.
          push.notify(notice("s0", "failed"));
          assert_eq!(notices.try_recv().map(|n| n.body), Some("failed".into()));
          assert_eq!(std::iter::from_fn(|| notices.try_recv()).count(), NOTICE_QUEUE - 1);
      }

      #[test]
      fn without_a_reader_every_notice_is_dropped() {
          let push = Push::detached();
          push.notify(notice("s1", "finished"));
          let (push, notices) = Push::new();
          drop(notices);
          push.notify(notice("s1", "finished"));
          assert!(push.shared.queue.lock().unwrap().latest.is_empty());
      }

      #[tokio::test]
      async fn a_reader_waits_for_the_next_notice() {
          let (push, mut notices) = Push::new();
          let reader = tokio::spawn(async move { notices.recv().await });
          tokio::task::yield_now().await;
          push.notify(notice("s1", "finished"));
          assert_eq!(reader.await.unwrap().tag, "s1");
      }

  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  pub mod hub;
  pub mod offline;
  ```

with:

  ```rust
  pub mod hub;
  pub mod notify;
  pub mod offline;
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  use hennery_kernel::push::VapidKey;
  ```

with:

  ```rust
  use hennery_kernel::push::{Push, VapidKey};
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      pub vapid: Arc<VapidKey>,
  }
  ```

with:

  ```rust
      pub vapid: Arc<VapidKey>,
      /// Where push triggers send their notices (ACP core §10). `new` gives
      /// a queue nobody reads; the collector replaces it with one delivery
      /// drains.
      pub push: Push,
  }
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
              vapid: Arc::new(VapidKey::generate()),
          }
  ```

with:

  ```rust
              vapid: Arc::new(VapidKey::generate()),
              push: Push::detached(),
          }
  ```

Create `crates/hennery-sessions/src/notify.rs`:

  ```rust
  //! Push triggers (ACP core §10; plan 10b): which edges a fact crosses
  //! notify the owner, and with what. The store reports the edge
  //! (`store::PushEdge`), from the ingest of a host fact only; this decides,
  //! in one place, and the kernel applies the hat's policy and delivers
  //! (`hennery_kernel::push`).

  use crate::store::{EdgeSession, PushEdge, one_line};
  use hennery_kernel::push::{Notice, Urgency};
  use hennery_proto::frames::TurnOutcome;
  use hennery_proto::rest::{TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES};

  /// The notice an edge of `session` gives, `session` as the edge's fact left
  /// it (`store::Edge`), or `None` for an edge that does
  /// not notify:
  ///
  /// | Edge | Title / body |
  /// |---|---|
  /// | activity → `blocked` | the session's / "needs your answer" (urgent) |
  /// | `turn_ended{completed}` | the session's / "finished" |
  /// | `turn_ended{failed}` | the session's / "failed" |
  ///
  /// A cancelled or interrupted turn does not notify: the operator cancelled
  /// it, or the host's restart already shows on the session. A question asked
  /// **outside a turn** does not notify either, until the maintainer decides
  /// whether it should (plan 10b's open question): it leaves the activity
  /// alone, so it is not `blocked`. If it ever notifies, it needs a limit in
  /// time per session: nothing the operator does paces it (10b-i's review).
  pub fn notice_for(edge: &PushEdge, session: &EdgeSession) -> Option<Notice> {
      let (urgency, generic_title, body, detail) = match edge {
          PushEdge::Blocked { title, .. } => (
              Urgency::High,
              "Session needs your answer",
              "needs your answer",
              title.clone(),
          ),
          // The maintainer's open question: no trigger of its own yet.
          PushEdge::QuestionOutsideTurn { .. } => return None,
          PushEdge::TurnEnded(TurnOutcome::Completed) => (Urgency::Normal, "Session finished", "finished", None),
          PushEdge::TurnEnded(TurnOutcome::Failed) => (Urgency::Normal, "Session failed", "failed", None),
          PushEdge::TurnEnded(TurnOutcome::Cancelled | TurnOutcome::Interrupted) => return None,
      };
      Some(Notice {
          hat_id: session.hat_id.clone(),
          urgency,
          title: session_title(session),
          generic_title: generic_title.into(),
          body: body.into(),
          detail,
          url: format!("/sessions/{}", session.id),
          tag: session.id.clone(),
      })
  }

  /// The session's title, else the name of its project directory, else
  /// "Session" (ACP core §10). The directory's name is put on one line as a
  /// title is (10b-i's review, A4): it reaches a lock screen.
  fn session_title(session: &EdgeSession) -> String {
      session
          .title
          .clone()
          .or_else(|| {
              let name = std::path::Path::new(&session.cwd).file_name()?;
              one_line(&name.to_string_lossy(), TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES)
          })
          .unwrap_or_else(|| "Session".into())
  }
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
  use crate::store::Store;
  ```

with:

  ```rust
  use crate::store::{Edge, Ingested, PushEdge, Store};
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
      let mut reconciled = false;
      // `close_session` frames the reconciliation loop re-sends by itself
  ```

with:

  ```rust
      let mut reconciled = false;
      // The push edges of facts the host resent before its resend completed:
      // notified once reconciliation is done, if they still hold (10b-i's
      // review, A1). A resent question already withdrawn or answered must not
      // ask the owner. Per session, the latest of each kind that would notify
      // (A6): a later edge that notifies nothing must not hide a "finished".
      // Lost if the connection drops first: a push is not state.
      let mut deferred: HashMap<String, Deferred> = HashMap::new();
      // `close_session` frames the reconciliation loop re-sends by itself
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                  match state.store.ingest(&session_id, seq, &body) {
                      Ok(created) => {
  ```

with:

  ```rust
                  match state.store.ingest_fact(&session_id, seq, &body) {
                      Ok(Ingested { events: created, edge }) => {
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                          for event in created {
                              state.hub.publish(event);
                          }
                          if started {
  ```

with:

  ```rust
                          for event in created {
                              state.hub.publish(event);
                          }
                          // After the commit, and only from here: recovery
                          // and reconciliation never push (ACP core §10).
                          if let Some(edge) = edge {
                              if reconciled {
                                  notify(&state, &edge);
                              } else {
                                  deferred.entry(session_id.clone()).or_default().keep(edge);
                              }
                          }
                          if started {
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                          }
                          tracing::info!(%host_id, "host reconciled");
  ```

with:

  ```rust
                          }
                          for (_, held) in deferred.drain() {
                              if let Some(edge) = held.into_edge(&state) {
                                  notify(&state, &edge);
                              }
                          }
                          tracing::info!(%host_id, "host reconciled");
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust

  /// Remember the cwd of a session that just started or resumed as one of its
  ```

with:

  ```rust

  /// Notify the owner of `edge`, which a host fact crossed, if it notifies
  /// (`notify::notice_for`). Queued, never waited on.
  fn notify(state: &AppState, edge: &Edge) {
      if let Some(notice) = crate::notify::notice_for(&edge.kind, &edge.session) {
          state.push.notify(notice);
      }
  }

  /// A session's edges deferred during a resend (A1, A6): its latest
  /// question that blocked a turn, and its latest turn end that notifies.
  #[derive(Default)]
  struct Deferred {
      blocked: Option<Edge>,
      ended: Option<Edge>,
  }

  impl Deferred {
      /// Keep `edge` if it would notify; an edge that notifies nothing
      /// replaces nothing.
      fn keep(&mut self, edge: Edge) {
          if crate::notify::notice_for(&edge.kind, &edge.session).is_none() {
              return;
          }
          match edge.kind {
              PushEdge::Blocked { .. } => self.blocked = Some(edge),
              PushEdge::TurnEnded(_) => self.ended = Some(edge),
              // Notifies nothing yet (the maintainer's open question): if it
              // ever does, it needs a slot and a check of its own here.
              PushEdge::QuestionOutsideTurn { .. } => {}
          }
      }

      /// What to notify once reconciled: the question if it is still open and
      /// its turn still blocked (`Store::still_blocked_on`), else the turn's
      /// end. A failed read is logged and counts as not holding: a push is not
      /// state.
      fn into_edge(self, state: &AppState) -> Option<Edge> {
          if let Some(edge) = self.blocked {
              let PushEdge::Blocked { pending_id, .. } = &edge.kind else {
                  unreachable!("`keep` files only `Blocked` here");
              };
              let holds = state
                  .store
                  .still_blocked_on(&edge.session.id, pending_id)
                  .unwrap_or_else(|err| {
                      tracing::warn!(session_id = %edge.session.id, error = %err, "deferred push dropped: store unreadable");
                      false
                  });
              if holds {
                  return Some(edge);
              }
          }
          self.ended
      }
  }

  /// Remember the cwd of a session that just started or resumed as one of its
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
          Undo::Prompt { session_id, turn_id } => store.abandon_turn(session_id, turn_id),
      }
  }
  ```

with:

  ```rust
          Undo::Prompt { session_id, turn_id } => store.abandon_turn(session_id, turn_id),
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::store::EdgeSession;
      use hennery_proto::frames::TurnOutcome;

      fn ended(outcome: TurnOutcome) -> Edge {
          Edge {
              kind: PushEdge::TurnEnded(outcome),
              session: EdgeSession {
                  id: "s1".into(),
                  hat_id: "hat-1".into(),
                  title: None,
                  cwd: "/p".into(),
              },
          }
      }

      /// A6: a later turn end that notifies nothing (cancelled, interrupted)
      /// does not replace one that does.
      #[test]
      fn a_quiet_turn_end_does_not_replace_a_finished_one() {
          let mut held = Deferred::default();
          held.keep(ended(TurnOutcome::Completed));
          held.keep(ended(TurnOutcome::Cancelled));
          held.keep(ended(TurnOutcome::Interrupted));
          assert_eq!(
              held.ended.map(|e| e.kind),
              Some(PushEdge::TurnEnded(TurnOutcome::Completed))
          );
      }
  }
  ```

- [ ] **Step 4: Run the tests, and the five checks**

  `push_edges` (14), `reconcile`'s five new tests and `push.rs`'s four queue tests pass. The socket tests passed in 3 rounds of 4 parallel copies (fleet rule).

- [ ] **Step 5: Revert-probe**

  | Change | Fails |
  |---|---|
  | drop `notify(&state, …)` in `ws.rs` | `a_blocked_turn_and_its_end_each_queue_one_notice` |
  | notify a resent edge at once, not deferred | `a_resent_backlog_notifies_only_what_still_holds` |
  | a deferred edge always held | the same |
  | the deferred edges never notified | `a_resent_question_still_open_notifies_after_reconciliation` |
  | an edge that notifies nothing deferred too | `a_quiet_turn_end_does_not_replace_a_finished_one` |
  | a waiting tag not replaced | `a_tag_waiting_keeps_its_place_and_takes_the_latest_notice` |
  | no `one_line` on the directory's name | `a_directorys_name_is_put_on_one_line` |
  | `QuestionOutsideTurn` notifying like `Blocked` | `a_question_outside_a_turn_does_not_notify_yet` |
  | `Cancelled` notifying | `only_blocked_finished_and_failed_notify` |
  | `Normal` for a blocked turn | the same |
  | the title from the cwd before the session title | `the_title_is_the_sessions_else_its_directorys` |
  | the full queue not counted | `a_full_queue_drops_new_tags_and_counts_them` |

- [ ] **Step 6: Commit**: `test(push): which edges notify, and that only a host's facts queue a notice`, then `feat(push): push triggers queue a notice for a blocked, finished or failed turn`.

## After this plan

For **10b-ii** (delivery):
- the collector's queue: `let (push, notices) = Push::new(); state.push = push;`, and a delivery task draining `notices`;
- apply `Notice`'s documented policy with `Hosts::push_policy(hat_id)`;
- the payload: `{title, body, url, tag}`, padded;
- `agent_failure`, once a host fills it (here, or in the profiles plan if it lands first): an edge `PushEdge::AgentFailure{severity}`, notifying "failed" unless the severity is `warning` (ACP core §10; missing or unknown severities escalate). Note its double notice with a `turn_ended{failed}` of the same turn: the same tag, so the newer replaces the older.

For **plan 8** (gateway): its `Notifier` calls `AppState::push.notify(Notice { url: "/mcp", tag: "mcp-<id>", urgency: Normal, … })`.

For **the binary**: `AppState::new` gives `Push::detached()`. 10b-ii must replace `state.push` before the collector serves, or every notice is dropped.

For **plan 4** (frontend):
- the service worker renders the payload's text fields as text only;
- it navigates only to a same-origin path the payload names (`/sessions/<id>`, `/mcp`), never to a URL;
- Settings warns, next to `details`, that a question's title can hold a command line (the review's O2).

Recorded, not done: a `host_session.rs` test that `session/load`'s replay creates no question or turn end. Questions are requests and a replay sends none, so neither edge can come from it (the review's O5).

For the **spec write-back**: ACP core §10 gains the edges as built (decisions 1–3) and the open question; §3.2's extract table gains `pending.title`.

---

_Generated with Claude AI — please review before distribution._
