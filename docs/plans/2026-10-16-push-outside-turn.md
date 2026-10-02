# Web Push (plan 10b-iii): a question outside a turn notifies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** a question (permission or elicitation) the agent opens while no turn is running notifies the owner "needs your answer", exactly like a turn blocked on its first question. **Operator decision 2026-10-02**, answering the open question plans 10a and 10b-i left.

**Architecture** (`hennery-sessions` only):
- **Sessions migration 11:** `pending.opened_event_id`, and the index `events_by_kind (session_id, kind, event_id)`.
- `notify::notice_for`: `PushEdge::QuestionOutsideTurn` gives `Blocked`'s notice: urgent, "needs your answer", the question's title as the `detail`.
- `store.rs`, the `pending_opened` arm:
  - a question outside a turn is quiet unless it is the session's only open question, and unless no question outside a turn was withdrawn by the agent since the owner's last prompt;
  - a question with no turn id that blocks a turn has the same bound;
  - `Store::still_open`.
- `ws.rs`, `Deferred`: one slot for the latest question of either kind, checked at the flush by `still_blocked_on` or `still_open`.

**Tech Stack:** Rust (edition 2024, MSRV 1.88). No new crates, no wire change. One sessions-store migration (11).

**Spec:** ACP core §10 (push triggers), §4.2; umbrella §5 ("a question the agent asks outside a turn is pending too, but leaves the activity alone"). It builds on plan [10b-i](2026-10-15-push-triggers.md), merged as #61: its decisions 1, 2 and 5 apply as they stand. Every anchor was taken from `main` at `ef75d4f` (10b-i and plan 8b merged).

**Status:** written 2026-10-02; executed 2026-10-02 (see "Execution status"). Amended after its review of 2026-10-02 (opus, on the maintainer's behalf): A1, A2 and N3 taken, N1 recorded. Its re-confirmation added A3 (taken).

**How the code blocks were made and checked:**
- The code was built and tested first; every block below was generated from its diff.
- The plan was replayed from its text onto `ef75d4f`, and the trees matched the tests-only and task commits byte for byte.
- The five checks passed: 1023 tests on `ef75d4f`, 4 of them new (2 in the store, 2 through a host socket).
- Every guard was revert-probed (Step 5): 11 probes, each failing its test.

## Execution status (2026-10-02)

**Executed** on `main` at `ef75d4f`. The code was built first, reviewed and amended, then cut into the task's two commits (tests first, then code), and the plan was replayed from its text until the trees matched.

| Area | As built | Why |
|---|---|---|
| The operator's decision (2026-10-02) | A question opened while no turn runs notifies "needs your answer", as a blocked turn does, under 10b-i's dedup and reconnect rules. | The open question plans 10a and 10b-i left. |
| Its review (opus, on the maintainer's behalf) | Confirmed with amendments. A1: a blocking question with no turn id is bounded by the owner's prompt too, closing 10b-i's N1. A2: the socket test ends on another session's notice, since the queue keeps one per tag. N3: the bound is ordered by event id, not the clock. N1 is recorded. | `NULL = NULL` left such a question unbounded. Millisecond stamps tied, and either reading of a tie misjudged one case. |
| Its re-confirmation | A3 taken: the question's own event id is stored (`pending.opened_event_id`, sessions migration 11), with an index on events by kind. The planner's use of the index for the `max()` was checked. Then "confirmed". | A first draft joined events on JSON, scanning the session's events per question under the write lock. |
| The whole-branch review (opus): approve with fixes | The owner's-prompt reset of a blocking question with no turn id is now tested (the no-turn-id test asks again after a new prompt), and its probe table row names the right test. `Deferred`'s doc and 10b-i's README line are current. | A reset clause no test failed without was unverified. |

Checks:
- The five checks passed: 1023 tests on `ef75d4f`.
- The 11 revert-probes each failed their test.
- The run was macOS only, so ubuntu CI is the Linux check.


## Scope

**1 task:** the trigger, its dedup, and its reconnect rule.

**Out:**
- delivery (10b-ii, waiting on plan 8b's egress);
- `agent_failure` (no host fills it yet; 10b-i's "After this plan").

## Decisions

1. **The trigger** (the operator's, 2026-10-02): a question opened while no turn runs notifies as a blocked turn does. That means the same urgency, text and `detail`, the same hat policy, and the same tag (the session), so a later notice replaces it on a device.
2. **The same dedup, applied outside a turn** (10b-i decision 1, which the operator asked to reuse):
   - **Edge-triggered:** only the session's first open question notifies. A second, while one is open, is quiet, as a second question in a blocked turn is.
   - **Paced by the owner** (10b-i's invariant: every edge the agent paces is bounded by something the owner paces).
     - A question asked again after the agent withdrew one outside a turn is quiet until the owner's next prompt.
     - "Since the owner's last prompt" means its question's `pending_opened` fact comes after the session's latest `user_turn` event. Only a turn the owner prompted writes `user_turn`, when it starts, so the bound is the owner's pace, never the agent's.
     - It is ordered by event id, which only grows, not by the clock (the review's N3). Millisecond stamps tied in tests, and either way a tie misread one case. A clock stepping back cannot reopen it either.
     - **The question's own event is stored with it** (the re-confirmation's A3): `pending.opened_event_id`, the `pending_opened` fact's id. The prompt's is found through `events_by_kind`. A first draft joined `events` on the question's JSON instead, which scanned every event of the session per question, under the write lock, and the agent can grow both. Questions from before the migration have no id, and never count as withdrawn since a prompt.
     - Without it, an agent could ask and withdraw in a loop between turns, each ask an urgent push. 10b-i's review (O1) asked for exactly such a bound if this ever notified.
     - Answered, then asked again, still notifies: that is the owner's pace.
   - It is read in the fact's transaction, from `pending` and `events`, owner-scoped.
   - **A question with no turn id that blocks a turn** gets the same bound (the review's A1, closing 10b-i's re-confirmation N1). In-turn withdrawals compare turn ids, and `NULL = NULL` is never true, so such a question escaped the bound until now.
3. **The same reconnect rule** (10b-i decision 2): a question resent before `resend_complete` is deferred, and notified after reconciliation only if it is still open (`Store::still_open`). `Deferred` keeps the latest question of either kind in one slot. A question that no longer holds falls back to the turn's end, so it never hides a "finished".
   - Recorded, not changed (the review's N1): with one slot, a later blocking question withdrawn in the same backlog hides an outside-turn question still open. The owner then gets "finished" instead of "needs your answer". It is rare, and only the urgency is lost.

## Global Constraints

- The five checks pass. No new crates; no wire change.
- The store's new statements name the owner (the owner audit reads `store.rs`).
- **Migration:** sessions migration 11 is new. The three tests that rewind the store's schema (`tests/owner.rs`, two in `tests/store.rs`) undo it too. Another lane adding a sessions migration renumbers at merge (fleet rule).
- No test reaches the network (#54): the socket tests are on loopback.
- Commits: Conventional Commits, gmail identity, unsigned.

## Review Focus

1. **A flood between turns.** Expected: one notice per open question; nothing for asks after a withdrawal until the owner's next prompt, with or without a turn id. Tests: `only_the_first_open_question_outside_a_turn_crosses_an_edge`, `a_question_asked_again_outside_a_turn_after_a_withdrawal_waits_for_the_owners_turn`, `a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked`, and `a_question_outside_a_turn_queues_one_urgent_notice`. The last one ends on another session's notice, since the queue keeps one notice per tag (the review's A2).
2. **A reconnect.** Expected: a resent question outside a turn notifies after reconciliation if it is still open, and not if it was withdrawn. Tests: `a_resent_question_outside_a_turn_still_open_notifies_after_reconciliation`, `a_resent_backlog_notifies_only_what_still_holds`.
3. **What it says.** Expected: as a blocked turn. Test: `a_question_outside_a_turn_notifies_like_a_blocked_turn`.

**Reading the steps.** Each block is one of:
- "Create `path`:" (a new file);
- "In `path`, replace:" with the exact text it replaces, which occurs once, then "with:".

Apply them in order.

---

### Task 1: A question outside a turn notifies

- [ ] **Step 1: Write the tests**

In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
      ));
  }
  ```

with:

  ```rust
      ));
      // Withdrawn, and asked again with no turn id: quiet, as in a turn
      // (plan 10b-iii's review, A1).
      use hennery_proto::frames::{PendingReason, PendingResolution};
      let withdrawn = SessionBody::PendingResolved {
          pending_id: "p1".into(),
          resolution: PendingResolution::Cancelled,
          reason: Some(PendingReason::AgentWithdrew),
      };
      store.ingest("s1", 4, &withdrawn).unwrap();
      assert_eq!(edge(&store, 5, &question("p2", None, None)), None);
      // The owner's next prompt resets the bound: withdrawn, the turn ended,
      // a new turn prompted, and a question with no turn id blocks it again.
      let withdrawn_p2 = SessionBody::PendingResolved {
          pending_id: "p2".into(),
          resolution: PendingResolution::Cancelled,
          reason: Some(PendingReason::AgentWithdrew),
      };
      store.ingest("s1", 6, &withdrawn_p2).unwrap();
      store.ingest("s1", 7, &ended("t1", TurnOutcome::Completed)).unwrap();
      assert!(
          store
              .open_turn("s1", "t2", &[json!({"type": "text", "text": "go on"})])
              .unwrap()
      );
      store
          .ingest(
              "s1",
              8,
              &SessionBody::TurnStarted {
                  request_id: "req-t2".into(),
                  turn_id: "t2".into(),
              },
          )
          .unwrap();
      assert!(matches!(
          edge(&store, 9, &question("p3", None, None)),
          Some(PushEdge::Blocked { .. })
      ));
  }
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

  /// `s1`, started, idle: no turn running.
  fn idle(store: &Store) {
      store
          .create_session("s1", "h1", "fake", "/home/me/project", "hat-1", None)
          .unwrap();
      store
          .ingest("s1", 1, &SessionBody::session_started("r0", "a1"))
          .unwrap();
  }

  /// Operator decision 2026-10-02, with 10b-i's dedup: outside a turn, only
  /// the session's first open question crosses an edge.
  #[test]
  fn only_the_first_open_question_outside_a_turn_crosses_an_edge() {
      let store = Store::open_in_memory().unwrap();
      idle(&store);
      assert!(matches!(
          edge(&store, 2, &question("p1", None, None)),
          Some(PushEdge::QuestionOutsideTurn { .. })
      ));
      assert_eq!(edge(&store, 3, &question("p2", None, None)), None);
  }

  /// Asked, withdrawn by the agent, asked again outside a turn: quiet, until
  /// the owner's next turn starts.
  #[test]
  fn a_question_asked_again_outside_a_turn_after_a_withdrawal_waits_for_the_owners_turn() {
      use hennery_proto::frames::{PendingReason, PendingResolution};
      let store = Store::open_in_memory().unwrap();
      idle(&store);
      let withdrawn = |id: &str| SessionBody::PendingResolved {
          pending_id: id.into(),
          resolution: PendingResolution::Cancelled,
          reason: Some(PendingReason::AgentWithdrew),
      };
      assert!(edge(&store, 2, &question("p1", None, None)).is_some());
      store.ingest("s1", 3, &withdrawn("p1")).unwrap();
      assert_eq!(edge(&store, 4, &question("p2", None, None)), None);
      store.ingest("s1", 5, &withdrawn("p2")).unwrap();
      // The owner's prompt: a turn, started and ended.
      assert!(
          store
              .open_turn("s1", "t1", &[json!({"type": "text", "text": "go on"})])
              .unwrap()
      );
      store
          .ingest(
              "s1",
              6,
              &SessionBody::TurnStarted {
                  request_id: "req-t1".into(),
                  turn_id: "t1".into(),
              },
          )
          .unwrap();
      store.ingest("s1", 7, &ended("t1", TurnOutcome::Completed)).unwrap();
      assert!(matches!(
          edge(&store, 8, &question("p3", None, None)),
          Some(PushEdge::QuestionOutsideTurn { .. })
      ));
  }
  ```

In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
  /// The maintainer's open question (plan 10b): until it is answered, a
  /// question asked outside a turn does not notify. Answering it changes
  /// `notice_for` and this test.
  #[test]
  fn a_question_outside_a_turn_does_not_notify_yet() {
  ```

with:

  ```rust
  /// Operator decision 2026-10-02: a question asked outside a turn notifies
  /// like a blocked turn.
  #[test]
  fn a_question_outside_a_turn_notifies_like_a_blocked_turn() {
  ```

In `crates/hennery-sessions/tests/push_edges.rs`, replace:

  ```rust
          title: None,
      };
      assert_eq!(notice_for(&edge, &session(&store)), None);
  ```

with:

  ```rust
          title: Some("Which branch?".into()),
      };
      let notice = notice_for(&edge, &session(&store)).unwrap();
      assert_eq!(
          (notice.urgency, notice.body.as_str(), notice.generic_title.as_str()),
          (Urgency::High, "needs your answer", "Session needs your answer")
      );
      assert_eq!(notice.detail.as_deref(), Some("Which branch?"));
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
      // A6: a later edge that notifies nothing (a question outside any turn)
      // must not hide the "finished".
      host.emit(&session, outside("p2")).await;
      wait_for("the backlog", || async {
          (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
  ```

with:

  ```rust
      // A question outside a turn, asked and withdrawn in the backlog: it no
      // longer holds at the flush, and must not hide the "finished" (A6).
      host.emit(&session, outside("p2")).await;
      host.emit(
          &session,
          SessionBody::PendingResolved {
              pending_id: "p2".into(),
              resolution: PendingResolution::Cancelled,
              reason: Some(PendingReason::AgentWithdrew),
          },
      )
      .await;
      wait_for("the backlog", || async {
          let resolved = collector
              .event_kinds(&session)
              .iter()
              .filter(|k| *k == "pending_resolved")
              .count();
          (resolved == 2).then_some(())
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
          payload,
      }
  }
  ```

with:

  ```rust
          payload,
      }
  }

  /// Operator decision 2026-10-02: a question asked while no turn runs asks
  /// the owner like a blocked turn; a second one while it is open does not.
  /// The queue keeps one notice per tag, so the second is checked by ending
  /// on another session's notice: none of this session's may come after the
  /// first (plan 10b-iii's review, A2).
  #[tokio::test]
  async fn a_question_outside_a_turn_queues_one_urgent_notice() {
      use hennery_kernel::push::Urgency;
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      host.emit(&session, outside("p1")).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
      assert_eq!(
          (notices[0].urgency, notices[0].tag.as_str()),
          (Urgency::High, session.as_str())
      );
      host.emit(&session, outside("p2")).await;
      // A sentinel on the same socket: another session's turn, blocked.
      let other = started_session(&collector, &mut host).await;
      let turn = started_turn(&collector, &mut host, &other).await;
      host.emit(&other, opened("q1", &turn)).await;
      let notices = collector.notices_until("needs your answer").await;
      assert!(notices.iter().all(|n| n.tag == other), "{notices:?}");
  }

  /// The same reconnect rule as a blocked turn: a resent question outside a
  /// turn notifies once reconciled, if it is still open.
  #[tokio::test]
  async fn a_resent_question_outside_a_turn_still_open_notifies_after_reconciliation() {
      let collector = Collector::start().await;
      let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
      let session = started_session(&collector, &mut host).await;
      let seq = host.seq;
      host.drop_connection(&collector).await;
      let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
      host.emit(&session, outside("p1")).await;
      wait_for("the question", || async {
          (!collector.state.store.open_pending(&session).unwrap().is_empty()).then_some(())
      })
      .await;
      assert!(collector.notices().is_empty());
      host.send(&HostFrame::ResendComplete).await;
      let notices = collector.notices_until("needs your answer").await;
      assert_eq!(notices.len(), 1, "{notices:?}");
  }
  ```

- [ ] **Step 2: Run them, and see them fail**

  `nix develop -c cargo test -p hennery-sessions --test push_edges` fails 4: `a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked`, `a_question_outside_a_turn_notifies_like_a_blocked_turn`, `only_the_first_open_question_outside_a_turn_crosses_an_edge` and `a_question_asked_again_outside_a_turn_after_a_withdrawal_waits_for_the_owners_turn`. `nix develop -c cargo test -p hennery-testkit --test reconcile` fails 2: `a_question_outside_a_turn_queues_one_urgent_notice` and `a_resent_question_outside_a_turn_still_open_notifies_after_reconciliation`. `a_resent_backlog_notifies_only_what_still_holds` passes either way.

- [ ] **Step 3: Flip the trigger, with its dedup and its reconnect rule**

In `crates/hennery-sessions/src/notify.rs`, replace:

  ```rust
  /// | activity → `blocked` | the session's / "needs your answer" (urgent) |
  /// | `turn_ended{completed}` | the session's / "finished" |
  ```

with:

  ```rust
  /// | activity → `blocked` | the session's / "needs your answer" (urgent) |
  /// | a question outside a turn | the same |
  /// | `turn_ended{completed}` | the session's / "finished" |
  ```

In `crates/hennery-sessions/src/notify.rs`, replace:

  ```rust
  /// **outside a turn** does not notify either, until the maintainer decides
  /// whether it should (plan 10b's open question): it leaves the activity
  /// alone, so it is not `blocked`. If it ever notifies, it needs a limit in
  /// time per session: nothing the operator does paces it (10b-i's review).
  pub fn notice_for(edge: &PushEdge, session: &EdgeSession) -> Option<Notice> {
      let (urgency, generic_title, body, detail) = match edge {
          PushEdge::Blocked { title, .. } => (
  ```

with:

  ```rust
  /// **outside a turn** leaves the activity alone, so it is not `blocked`, but
  /// it waits on the owner just the same: it notifies as one (operator
  /// decision 2026-10-02). The store bounds it by the owner's pace: only the
  /// session's first open question, and none after a withdrawal until the
  /// owner's next prompt.
  pub fn notice_for(edge: &PushEdge, session: &EdgeSession) -> Option<Notice> {
      let (urgency, generic_title, body, detail) = match edge {
          PushEdge::Blocked { title, .. } | PushEdge::QuestionOutsideTurn { title, .. } => (
  ```

In `crates/hennery-sessions/src/notify.rs`, replace:

  ```rust
          // The maintainer's open question: no trigger of its own yet.
          PushEdge::QuestionOutsideTurn { .. } => return None,
  ```

with:

  ```rust
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      CREATE INDEX sessions_by_hat ON sessions(owner_id, hat_id, last_event_at DESC, id DESC);
  ",
  ```

with:

  ```rust
      CREATE INDEX sessions_by_hat ON sessions(owner_id, hat_id, last_event_at DESC, id DESC);
  ",
      // Web Push (plan 10b-iii; its review's A3): the event that opened each
      // question, so a withdrawal is ordered against the owner's last prompt
      // by id, without a scan of the session's events; and the index that
      // finds a session's latest event of a kind. Questions from before have
      // none, and never count as withdrawn since a prompt.
      "
      ALTER TABLE pending ADD COLUMN opened_event_id INTEGER;
      CREATE INDEX events_by_kind ON events(session_id, kind, event_id);
  ",
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                                               owner_id)
                           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8)
  ```

with:

  ```rust
                                               owner_id, opened_event_id)
                           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8, ?9)
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                              self.owner
  ```

with:

  ```rust
                              self.owner,
                              fact_id
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      let rewithdrawn = blocked > 0
                          && tx.query_row(
                              "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND turn_id = ?2
                                   AND pending_id <> ?3 AND reason = 'agent_withdrew' AND owner_id = ?4)",
                              params![session_id, indexed.turn_id, pending_id, self.owner],
                              |r| r.get::<_, bool>(0),
                          )?;
                      edge = if rewithdrawn {
  ```

with:

  ```rust
                      //
                      // A question with no turn id, in a turn or outside one,
                      // is bounded by the owner's last prompt instead (10b-i's
                      // re-confirmation, N1; plan 10b-iii's review, A1): a
                      // withdrawal counts if its question was opened after
                      // the session's latest `user_turn`, which only a turn
                      // the owner prompted writes. Ordered by event id, which
                      // only grows, not by the clock (the review's N3): the
                      // question's own (`opened_event_id`) against the
                      // prompt's, found by `events_by_kind` (A3).
                      let rewithdrawn = blocked > 0
                          && tx.query_row(
                              "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND pending_id <> ?3
                                   AND reason = 'agent_withdrew' AND turn_id = ?2 AND owner_id = ?4)
                               OR (?2 IS NULL AND EXISTS(SELECT 1 FROM pending WHERE session_id = ?1
                                   AND pending_id <> ?3 AND owner_id = ?4 AND turn_id IS NULL
                                   AND reason = 'agent_withdrew'
                                   AND opened_event_id > coalesce((SELECT max(event_id) FROM events
                                       WHERE session_id = ?1 AND kind = 'user_turn' AND owner_id = ?4), 0)))",
                              params![session_id, indexed.turn_id, pending_id, self.owner],
                              |r| r.get::<_, bool>(0),
                          )?;
                      // Outside a turn the same two rules hold (operator
                      // decision 2026-10-02): only the first open question
                      // notifies, and one asked again after the agent withdrew
                      // one is quiet until the owner's next turn, bounded as
                      // above.
                      let outside_quiet = blocked == 0
                          && indexed.turn_id.is_none()
                          && tx.query_row(
                              "SELECT EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND pending_id <> ?2
                                   AND state = 'open' AND owner_id = ?3)
                               OR EXISTS(SELECT 1 FROM pending WHERE session_id = ?1 AND pending_id <> ?2
                                   AND owner_id = ?3 AND turn_id IS NULL AND reason = 'agent_withdrew'
                                   AND opened_event_id > coalesce((SELECT max(event_id) FROM events
                                       WHERE session_id = ?1 AND kind = 'user_turn' AND owner_id = ?3), 0))",
                              params![session_id, pending_id, self.owner],
                              |r| r.get::<_, bool>(0),
                          )?;
                      edge = if rewithdrawn || outside_quiet {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          Ok(Ingested { events: created, edge })
      }
  ```

with:

  ```rust
          Ok(Ingested { events: created, edge })
      }

      /// Whether `pending_id` is still open in `session_id`: a question asked
      /// outside a turn, resent before reconnecting, is notified only if it
      /// is (operator decision 2026-10-02).
      pub fn still_open(&self, session_id: &str, pending_id: &str) -> Result<bool> {
          Ok(self.conn().query_row(
              "SELECT EXISTS(SELECT 1 FROM pending WHERE pending_id = ?1 AND session_id = ?2 AND state = 'open'
                   AND owner_id = ?3)",
              [pending_id, session_id, &self.owner],
              |r| r.get(0),
          )?)
      }
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
  /// A session's edges deferred during a resend (A1, A6): its latest
  /// question that blocked a turn, and its latest turn end that notifies.
  #[derive(Default)]
  struct Deferred {
      blocked: Option<Edge>,
  ```

with:

  ```rust
  /// A session's edges deferred during a resend (A1, A6): its latest question
  /// (one that blocked a turn, or one asked outside a turn), and its latest
  /// turn end that notifies.
  #[derive(Default)]
  struct Deferred {
      /// The latest question: one that blocked a turn, or one asked outside a
      /// turn.
      asked: Option<Edge>,
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
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
  ```

with:

  ```rust
              PushEdge::Blocked { .. } | PushEdge::QuestionOutsideTurn { .. } => self.asked = Some(edge),
              PushEdge::TurnEnded(_) => self.ended = Some(edge),
          }
      }

      /// What to notify once reconciled: the question if it still holds (one
      /// that blocked a turn: still open and the turn still blocked,
      /// `Store::still_blocked_on`; one outside a turn: still open,
      /// `Store::still_open`), else the turn's end. A failed read is logged
      /// and counts as not holding: a push is not state.
      fn into_edge(self, state: &AppState) -> Option<Edge> {
          if let Some(edge) = self.asked {
              let id = &edge.session.id;
              let held = match &edge.kind {
                  PushEdge::Blocked { pending_id, .. } => state.store.still_blocked_on(id, pending_id),
                  PushEdge::QuestionOutsideTurn { pending_id, .. } => state.store.still_open(id, pending_id),
                  PushEdge::TurnEnded(_) => unreachable!("`keep` files only questions here"),
              };
              let holds = held.unwrap_or_else(|err| {
                  tracing::warn!(session_id = %edge.session.id, error = %err, "deferred push dropped: store unreadable");
                  false
              });
  ```

In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
          // 5c's migration (version 10) and 6b's (version 9) undone first,
          // their indexes on `owner_id` included.
          conn.execute_batch(
              "
  ```

with:

  ```rust
          // 10b-iii's migration (version 11), 5c's (version 10) and 6b's
          // (version 9) undone first, their indexes on `owner_id` included.
          conn.execute_batch(
              "
              DROP INDEX events_by_kind;
              ALTER TABLE pending DROP COLUMN opened_event_id;
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
              "DROP INDEX sessions_by_hat;
  ```

with:

  ```rust
              "DROP INDEX events_by_kind;
               ALTER TABLE pending DROP COLUMN opened_event_id;
               DROP INDEX sessions_by_hat;
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
      // Back to the store's schema before hats (version 9).
      let conn = rusqlite::Connection::open(&db).unwrap();
      conn.execute_batch(
          "DROP INDEX sessions_by_hat;
  ```

with:

  ```rust
      // Back to the store's schema before hats (version 9): plan 10b-iii's
      // migration (version 11) undone too.
      let conn = rusqlite::Connection::open(&db).unwrap();
      conn.execute_batch(
          "DROP INDEX events_by_kind;
           ALTER TABLE pending DROP COLUMN opened_event_id;
           DROP INDEX sessions_by_hat;
  ```

- [ ] **Step 4: Run the tests, and the five checks**

- [ ] **Step 5: Revert-probe**

  | Change | Fails |
  |---|---|
  | `QuestionOutsideTurn` back to no notice | `a_question_outside_a_turn_notifies_like_a_blocked_turn`, `a_question_outside_a_turn_queues_one_urgent_notice` |
  | drop `outside_quiet` | `only_the_first_open_question_outside_a_turn_crosses_an_edge` |
  | withdrawals outside a turn not counted | `a_question_asked_again_outside_a_turn_after_a_withdrawal_waits_for_the_owners_turn` |
  | the opening event id not stored | `a_question_asked_again_outside_a_turn_after_a_withdrawal_waits_for_the_owners_turn` |
  | a blocking question with no turn id unbounded | `a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked` |
  | the owner's prompt not resetting it, outside a turn | `a_question_asked_again_outside_a_turn_after_a_withdrawal_waits_for_the_owners_turn` |
  | the owner's prompt not resetting it, in a turn | `a_question_without_a_turn_id_that_blocks_a_running_turn_is_blocked` |
  | the question not deferred | `a_resent_question_outside_a_turn_still_open_notifies_after_reconciliation` |
  | a resent question notified at once | the same |
  | `still_open` always true | `a_resent_backlog_notifies_only_what_still_holds` |

- [ ] **Step 6: Commit**: `test(push): a question outside a turn notifies, once, and waits for reconciliation`, then `feat(push): a question asked outside a turn notifies like a blocked turn`.

## After this plan

- **For the spec write-back:** ACP core §10's table gains "a question outside a turn: `<session title>` / needs your answer", with decision 2's bound. The umbrella §5 note ("leaves the activity alone") stays true: the activity is unchanged; only the push is new.
- **For plan 4 (frontend):** nothing new; the notice is a blocked turn's.

---

_Generated with Claude AI — please review before distribution._
