# Hats (plan 5d): re-assignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The operator can move a session to another hat (ACP core §4.9; umbrella §8.2).
- `PATCH /api/sessions/{id} {hat_id}` re-assigns a session only while it has no running adapter.
- A presumed-parked session is refused: its host may still run it, so close it first.
- It needs a fresh step-up. It writes `hat_reassigned{from, to}`.
- The next resume must agree with the new hat.
- A closed and re-assigned session that its host still has attached is closed on that host at its next handshake; a test pins that.

This completes plan 5's sessions side.

**Architecture:**
- **Sessions** (`hennery-sessions`):
  - `store.rs`: `Store::reassign_hat(session_id, hat_id) -> Reassign {Done(EventDto), Unchanged, Attached(String), UnknownHat, NotFound}`, one transaction. The hat is checked as the owner's in that transaction.
  - `api.rs`: `PATCH /api/sessions/{id}` with optional fields (plan 6 may add `title`); step-up only when `hat_id` is given.
- **Kernel** (`auth.rs`): `step_up_required()`, the 403 the step-up layer answers, for a handler that needs step-up for only part of what it does.
- **Wire:** `UpdateSessionRequest {hat_id?}`.
- **Tests:**
  - the store's re-assignment;
  - over HTTP, against a scripted host, through resume;
  - the reconciliation close;
  - the step-up row, and a PATCH with no hat needing none.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. No new crates.

**Spec:** [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md) and the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md):
- ACP core §4.9: "Allowed for any session with no running adapter (`parked`, `closed` or `failed`), with a warning in the UI that the agent's stored history came from the old hat; the change writes a `hat_reassigned{from, to}` event. The next resume re-resolves the path and must agree with the new hat, or the operator must also change the path rules."
- ACP core §5.1: "attached sessions the collector has closed, or whose hat was re-assigned, receive `close_session`".
- ACP core §9: `PATCH /api/sessions/{id}`, "Rename; hat re-assignment (no running adapter, §4.9)".
- umbrella §8.2: "Re-assigning a session is allowed only while it is parked, with a warning, and is logged as a timeline event."

It builds on the executed plans [5a](2026-10-06-hats.md), [5b](2026-10-07-resolve-path.md) and [5c](2026-10-08-session-hats.md), and on plan 6b's session list. Every anchor was taken from the tree 5c leaves on `main`.

**Status:** not executed; amended after the security review.

The security review of 2026-10-02 covered 5b, 5c and 5d. It approved after amendments, then re-confirmed "confirmed with notes"; 5d itself needed none.

**How the code blocks were made and checked:**
- Every block below was generated from the reviewed code.
- The plan was replayed from its own text onto `3f666f1` (`main` with 5c merged). The tree matched the tests-only and task commits, byte for byte.
- After every task the five checks passed: 880 and 882 tests, from 879.
- The guards were revert-probed.

## Scope

Plans A and B1 hand on "close on re-assignment at `hello`". 5c hands on: re-assignment needs step-up, only with no running adapter, and checks the hat as the owner's.

That is **2 tasks**:
1. re-assigning a session in the store;
2. `PATCH /api/sessions/{id}`, behind step-up.

**Out:**
- revoking the session's gateway token on re-assignment (plan 8);
- the UI's warning that the history came from the old hat (plan 4);
- renaming a session (`title`), which joins the same PATCH later.

## Decisions this plan makes where the spec is silent

The security review of 2026-10-02 confirmed these on the maintainer's behalf.

1. **No running adapter, and a presumed-parked session counts as running.** (amendment, ACP core §5.1)
   - `parked` (not presumed), `closed` and `failed` may move. `starting` and `active` are 409 with their lifecycle as the code.
   - A presumed-parked session is 409 `presumed_parked`: "its host has been away and may still run it: close the session first".
   - Its adapter may still run the old hat's MCP servers on its host. Closing it first is immediate, and a closed session that its host still has attached is closed at the host's next handshake. That is ACP core §5.1's existing step, now pinned by a test.
   - So §5.1's "whose hat was re-assigned" case becomes unreachable, rather than handled.
2. **Step-up, in the handler, only when `hat_id` is given.**
   - 403 `step_up_required`, before any write.
   - The PATCH has optional fields, so a later `title` needs no step-up.
   - Why: re-assignment brings a session's history into another hat's reach, and with the gateway, its grants.
3. **The write.**
   - One transaction. The hat must be the owner's: an `EXISTS` on `hats`, owner-filtered; otherwise 400 `invalid`.
   - The same hat is a no-op 200.
   - Otherwise it writes `hat_reassigned{from, to}` (published on the session stream) and clears `hat_rule_id`: that rule no longer decided the hat.
   - The `UPDATE` re-checks the lifecycle in its `WHERE`, and must change exactly one row, or the transaction rolls back with no event (the port review's hardening).
   - The answer is 200 with the session's detail. The session list's `?hat=` (5c) shows it under its new hat at once.
   - A PATCH naming no hat (`{}`) is the detail, with no step-up.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task the five checks pass (fmt, both clippy runs, the workspace tests, `gen --check`).
- **No new crates.** **Wire types change in Task 2**; regenerate there.
- Every query names the owner; `store.rs` stays in the audit.
- Every new route is an operator's (`operator_only`), and takes its body as `ApiJson`.
- **No Linux-only code**, and no test that reads another process's state without polling for a positive signal (fleet rule).
- Commits: Conventional Commits, gmail identity, unsigned. Push after every task; never push `main`.

## Review Focus

1. **Re-assigning a session whose host is away** (presumed parked).
   - Expected: refused, saying to close it first. Closed, it moves. When the host returns, its adapter is closed.
   - Tests: Task 1 `a_session_with_no_running_adapter_is_reassigned_to_another_hat`; Task 2 `a_session_closed_and_reassigned_while_its_host_was_away_is_closed_on_its_return`.
2. **Re-assigning, then resuming before the rules agree.**
   - Expected: `hat_mismatch` until the rules agree, then the resume goes on in the new hat.
   - Tests: Task 2 `a_reassigned_session_resumes_in_its_new_hat_once_the_rules_agree`, which also checks the event is published on the stream and the list's `?hat=` moves.
3. **A stale session re-assigning.**
   - Expected: 403 `step_up_required` before anything is read or changed.
   - Tests: Task 2's step-up row.
4. **Another owner's hat, or one that does not exist.**
   - Expected: 400 `invalid`, nothing written.
   - Tests: Task 1, the `UnknownHat` cases: a made-up hat and another owner's, with no event written.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-sessions/src/store.rs` | `Reassign`, `reassign_hat` | 1 |
| `crates/hennery-kernel/src/auth.rs` | `step_up_required` | 2 |
| `crates/hennery-sessions/src/api.rs` | `PATCH /api/sessions/{id}` | 2 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs`, generated files | `UpdateSessionRequest` | 2 |
| Tests: `crates/hennery-sessions/tests/store.rs`; `crates/hennery-testkit/tests/{resolve,reconcile,step_up}.rs` | | 1, 2 |

**Reading the steps:** as in plan 5a.

---

### Task 1: Re-assigning a session in the store

**Files:**
- Modify: `crates/hennery-sessions/src/store.rs`
- Test: `crates/hennery-sessions/tests/store.rs`

**Interfaces:**
- Produces: `hennery_sessions::store::Reassign`, and `Store::reassign_hat(session_id, hat_id) -> Result<Reassign>`.
- Consumes: 5c's `sessions.hat_id` and `hat_rule_id`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/tests/store.rs`, replace:

```rust
    assert_eq!(rest.next_cursor, None);
}
```

with:

```rust
    assert_eq!(rest.next_cursor, None);
}

/// ACP core §4.9, plan 5d decision 1: a session with no running adapter
/// moves to another of the owner's hats, with a `hat_reassigned` event;
/// one that may still run (`starting`, `active`, or presumed parked while
/// its host is away) does not, nor to a hat that is not the owner's.
#[test]
fn a_session_with_no_running_adapter_is_reassigned_to_another_hat() {
    use hennery_kernel::hats::HatChange;
    use hennery_kernel::hosts::Hosts;
    use hennery_sessions::store::Reassign;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let personal = hosts.default_hat_for_new_hosts().unwrap();
    let HatChange::Done(acme) = hosts.create_hat("Acme", None, 1).unwrap() else {
        panic!("no hat");
    };
    let store = Store::open(&db).unwrap();
    store
        .create_session("s1", "h1", "fake", "/tmp", &personal, Some("rule-1"))
        .unwrap();
    // `starting`, then `active`: an adapter may run.
    assert_eq!(
        store.reassign_hat("s1", &acme.id).unwrap(),
        Reassign::Attached("starting".into())
    );
    store
        .ingest("s1", 1, &SessionBody::session_started("r1", "agent-1"))
        .unwrap();
    assert_eq!(
        store.reassign_hat("s1", &acme.id).unwrap(),
        Reassign::Attached("active".into())
    );
    // Presumed parked: its host is away and may still run it.
    store.presume_parked("h1").unwrap();
    assert_eq!(
        store.reassign_hat("s1", &acme.id).unwrap(),
        Reassign::Attached("presumed_parked".into())
    );
    store.close_now("s1").unwrap();

    assert_eq!(store.reassign_hat("s1", "hat-nope").unwrap(), Reassign::UnknownHat);
    // Nor to another owner's hat.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO owners(id, created_at, set_up_at) VALUES ('owner-00000000000000b2', 9223372036854775807, 9223372036854775807);
         INSERT INTO hats(id, owner_id, name, colour, created_at)
             VALUES ('hat-theirs', 'owner-00000000000000b2', 'Theirs', '#000000', 9);",
    )
    .unwrap();
    let events = store.events("s1", 0, 100).unwrap().len();
    assert_eq!(store.reassign_hat("s1", "hat-theirs").unwrap(), Reassign::UnknownHat);
    assert_eq!(store.events("s1", 0, 100).unwrap().len(), events, "nothing written");
    assert_eq!(store.reassign_hat("s1", &personal).unwrap(), Reassign::Unchanged);
    let Reassign::Done(event) = store.reassign_hat("s1", &acme.id).unwrap() else {
        panic!("not re-assigned");
    };
    assert_eq!(
        (event.kind.as_str(), event.body.clone()),
        ("hat_reassigned", json!({ "from": personal, "to": acme.id }))
    );
    assert_eq!(store.session("s1").unwrap().unwrap().hat_id, acme.id);
    let rule: Option<String> = conn
        .query_row("SELECT hat_rule_id FROM sessions WHERE id = 's1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rule, None, "the rule that decided the old hat no longer does");
    assert_eq!(store.reassign_hat("s-nope", &acme.id).unwrap(), Reassign::NotFound);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-sessions --locked --test store`
Expected: FAIL to compile: `unresolved import hennery_sessions::store::Reassign`, `no method named reassign_hat found for struct Store`.

- [ ] **Step 3: The re-assignment**

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    HatMismatch(String),
```

with:

```rust
    HatMismatch(String),
    NotFound,
}

/// The outcome of `Store::reassign_hat` (ACP core §4.9).
#[derive(Debug, PartialEq)]
pub enum Reassign {
    /// Its `hat_reassigned` event.
    Done(EventDto),
    /// It is in that hat already: nothing written.
    Unchanged,
    /// It may have a running adapter (this lifecycle, or `presumed_parked`
    /// while its host is away): refused (plan 5d decision 1).
    Attached(String),
    /// No such hat of the owner's.
    UnknownHat,
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust

    /// The host has been offline past the threshold: presume its `active`
```

with:

```rust

    /// Move a session with no running adapter to another hat (ACP core
    /// §4.9): `parked` (and not only presumed so), `closed` or `failed`. The
    /// hat must be the owner's. Writes `hat_reassigned{from, to}`. The rule
    /// that decided the old hat no longer did, so it is cleared. The next
    /// resume must agree with the new hat (`hat_mismatch` otherwise).
    pub fn reassign_hat(&self, session_id: &str, hat_id: &str) -> Result<Reassign> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let row: Option<(String, bool, String)> = tx
            .query_row(
                "SELECT lifecycle, presumed_parked, hat_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
                [session_id, &self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((lifecycle, presumed, from)) = row else {
            return Ok(Reassign::NotFound);
        };
        if presumed {
            return Ok(Reassign::Attached("presumed_parked".into()));
        }
        if !matches!(lifecycle.as_str(), "parked" | "closed" | "failed") {
            return Ok(Reassign::Attached(lifecycle));
        }
        let known = tx
            .query_row(
                "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2",
                [hat_id, &self.owner],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !known {
            return Ok(Reassign::UnknownHat);
        }
        if from == hat_id {
            return Ok(Reassign::Unchanged);
        }
        let changed = tx.execute(
            "UPDATE sessions SET hat_id = ?2, hat_rule_id = NULL
             WHERE id = ?1 AND lifecycle IN ('parked', 'closed', 'failed') AND presumed_parked = 0 AND owner_id = ?3",
            params![session_id, hat_id, self.owner],
        )?;
        // The checks above read the same row in this transaction; an event
        // without the change it records would be worse than an error.
        anyhow::ensure!(changed == 1, "re-assigning {session_id} changed {changed} rows");
        let event = collector_event(
            &tx,
            &self.owner,
            session_id,
            "hat_reassigned",
            json!({ "from": from, "to": hat_id }),
            &now(),
        )?;
        tx.commit()?;
        Ok(Reassign::Done(event))
    }

    /// The host has been offline past the threshold: presume its `active`
```

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-sessions --locked --test store a_session_with_no_running_adapter`
Expected: PASS, 1 test.

- [ ] **Step 5: Revert-probes**

Each is run on the task's code, then restored.
- In `reassign_hat`, remove the `if presumed` refusal. The test fails on `presumed_parked`.
- Remove the `known` check. The test fails on `UnknownHat`.
- Drop `owner_id = ?2` from the hat check. The test fails on another owner's hat.

- [ ] **Step 6: The full checks**

Expected: all pass; **880 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates/hennery-sessions
git commit -m "feat(sessions): re-assign a session with no running adapter to another hat"
```

### Task 2: `PATCH /api/sessions/{id}`, behind step-up

**Files:**
- Modify: `crates/hennery-kernel/src/auth.rs`, `crates/hennery-sessions/src/api.rs`, `crates/hennery-proto/src/{rest,codegen}.rs`, the generated files
- Test: `crates/hennery-testkit/tests/{resolve,reconcile,step_up}.rs`

**Interfaces:**
- Produces: `PATCH /api/sessions/{id}` `UpdateSessionRequest {hat_id?}` → 200 `SessionDetail` | 400 | 403 `step_up_required` | 404 | 409 (lifecycle or `presumed_parked`); and `hennery_kernel::auth::step_up_required() -> Response`.
- Consumes: Task 1's `reassign_hat`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    assert_eq!(list(&format!("?q={}", "q".repeat(200))).await.0, 200);
}
```

with:

```rust
    assert_eq!(list(&format!("?q={}", "q".repeat(200))).await.0, 200);
}

/// ACP core §5.1: an attached session the collector has closed, and then
/// re-assigned to another hat while its host was away, is closed on that
/// host when it comes back, so no adapter of the old hat runs on.
#[tokio::test]
async fn a_session_closed_and_reassigned_while_its_host_was_away_is_closed_on_its_return() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    let (status, body) = post(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/close")),
        json!({}),
    )
    .await;
    assert_eq!((status, body["lifecycle"].as_str()), (202, Some("closed")), "{body}");
    let hennery_kernel::hats::HatChange::Done(acme) = collector.state.hosts.create_hat("Acme", None, 1).unwrap() else {
        panic!("no hat");
    };
    let resp = client(&collector)
        .patch(collector.url(&format!("/api/sessions/{session}")))
        .json(&json!({ "hat_id": acme.id }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let CollectorFrame::CloseSession { session_id, .. } = host.next().await else {
        panic!("expected the reconcile-driven close_session");
    };
    assert_eq!(session_id, session);
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.hat_id), ("closed", acme.id));
}
```

In `crates/hennery-testkit/tests/resolve.rs`, replace:

```rust
    assert_eq!(status, 202, "{body}");
}
```

with:

```rust
    assert_eq!(status, 202, "{body}");
}

/// ACP core §4.9: re-assigning a parked session moves it to another hat,
/// with a `hat_reassigned` event; its next resume must agree with the new
/// hat. A session in another lifecycle is refused, and so is a stale
/// step-up (`step_up.rs`).
#[tokio::test]
async fn a_reassigned_session_resumes_in_its_new_hat_once_the_rules_agree() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector).await;
    let call = start(&collector, "/home/me/acme");
    host.answer("/home/me/acme", true).await;
    let (session, _) = host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
    let acme = collector.hat("Acme");
    // The session list, by hat: (session id, hat id).
    let listed = |hat: String| {
        let call = send(&collector, "GET", &format!("/api/sessions?hat={hat}"), json!({}));
        async move {
            let (status, body) = call.await.unwrap();
            assert_eq!(status, 200, "{body}");
            body["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    (
                        s["session_id"].as_str().unwrap().to_string(),
                        s["hat_id"].as_str().unwrap().to_string(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };
    let default = collector.state.hosts.host(HOST).unwrap().unwrap().default_hat_id;
    assert_eq!(listed(default.clone()).await, [(session.clone(), default.clone())]);
    assert!(listed(acme.clone()).await.is_empty());
    let patch = |hat: &str| {
        send(
            &collector,
            "PATCH",
            &format!("/api/sessions/{session}"),
            json!({ "hat_id": hat }),
        )
    };
    let (status, body) = patch(&acme).await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("active")), "{body}");

    host.parked(&session).await;
    wait_for("parked", || async {
        let row = collector.state.store.session(&session).unwrap().unwrap();
        (row.lifecycle == "parked").then_some(())
    })
    .await;
    let mut stream = collector.state.hub.subscribe();
    let (status, body) = patch(&acme).await.unwrap();
    assert_eq!((status, body["hat_id"].as_str()), (200, Some(acme.as_str())), "{body}");
    // Published on the session's stream, not only stored.
    let published = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let event = stream.recv().await.unwrap();
            if event.session_id == session && event.kind == "hat_reassigned" {
                return event.body;
            }
        }
    })
    .await
    .expect("hat_reassigned published within 10s");
    assert_eq!(published, json!({ "from": default, "to": acme }));
    let kinds: Vec<String> = collector
        .state
        .store
        .events(&session, 0, 100)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds.last().map(String::as_str), Some("hat_reassigned"));
    // The session list shows it under its new hat, and no longer the old.
    assert_eq!(listed(acme.clone()).await, [(session.clone(), acme.clone())]);
    assert!(listed(default).await.is_empty());

    // The path still resolves to the host's default hat: refused.
    let resume = || {
        send(
            &collector,
            "POST",
            &format!("/api/sessions/{session}/resume"),
            json!({}),
        )
    };
    let call = resume();
    host.answer("/home/me/acme", true).await;
    let (status, body) = call.await.unwrap();
    assert_eq!((status, body["code"].as_str()), (409, Some("hat_mismatch")), "{body}");
    // Once a rule agrees, it resumes.
    rule(&collector, "/home/me/acme", &acme, true);
    let call = resume();
    host.answer("/home/me/acme", true).await;
    host.started().await;
    assert_eq!(call.await.unwrap().0, 202);
}
```

In `crates/hennery-testkit/tests/step_up.rs`, replace:

```rust
//! rules, changing a hat (plan 5a decisions 7 and 8) and revoking a session
//! need a password check within the last five minutes, and are refused
//! without one, accepted within five minutes, and refused after.
```

with:

```rust
//! rules, changing a hat (plan 5a decisions 7 and 8), re-assigning a
//! session to another hat (plan 5d decision 2) and revoking a session need
//! a password check within the last five minutes, and are refused without
//! one, accepted within five minutes, and refused after.
```

In `crates/hennery-testkit/tests/step_up.rs`, replace:

```rust
        ("PATCH", "/api/hats/hat-9".to_string(), Some(r#"{"name":"x"}"#), 404),
```

with:

```rust
        ("PATCH", "/api/hats/hat-9".to_string(), Some(r#"{"name":"x"}"#), 404),
        (
            "PATCH",
            "/api/sessions/s-9".to_string(),
            Some(r#"{"hat_id":"hat-9"}"#),
            404,
        ),
```

In `crates/hennery-testkit/tests/step_up.rs`, replace:

```rust
    }
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
```

with:

```rust
    }
    // A PATCH that names no hat needs no step-up (plan 5d decision 2).
    let resp = send(&stale, "PATCH", "/api/sessions/s-9", Some("{}")).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --locked --test resolve a_reassigned`
Expected: FAIL: `left: (405, None)`, `right: (409, Some("active"))`. There is no PATCH route yet.

- [ ] **Step 3: The route**

In `crates/hennery-kernel/src/auth.rs`, replace:

```rust
        return error(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "confirm your password or a passkey again (POST /api/auth/step-up/password or /api/auth/step-up/passkey/start)",
        );
    }
    next.run(req).await
```

with:

```rust
        return step_up_required();
    }
    next.run(req).await
}

/// 403 `step_up_required`: what `require_step_up` answers, for a handler
/// that needs a fresh step-up for only some of what it does.
pub fn step_up_required() -> Response {
    error(
        StatusCode::FORBIDDEN,
        "step_up_required",
        "confirm your password or a passkey again (POST /api/auth/step-up/password or /api/auth/step-up/passkey/start)",
    )
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::HatResolveRequest,
        rest::HatResolution,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::HatResolveRequest,
        rest::HatResolution,
        rest::UpdateSessionRequest,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::HatResolution,
    );
```

with:

```rust
        rest::HatResolution,
        rest::UpdateSessionRequest,
    );
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
    pub rule_id: Option<String>,
}
```

with:

```rust
    pub rule_id: Option<String>,
}

/// `PATCH /api/sessions/{id}` (ACP core §9): absent fields stay as they are.
/// `hat_id` re-assigns the session (ACP core §4.9): only with no running
/// adapter, and with a fresh step-up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct UpdateSessionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub hat_id: Option<String>,
}
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    AnswerSubmission, Cursor, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, ResumeRequest, Store,
```

with:

```rust
    AnswerSubmission, Cursor, LIFECYCLES, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT, ListQuery, Reassign, ResumeRequest, Store,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use hennery_kernel::operator::Authenticated;
```

with:

```rust
use hennery_kernel::operator::Authenticated;
use hennery_kernel::secret::unix_now;
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    StartSessionResponse, json_width,
```

with:

```rust
    StartSessionResponse, UpdateSessionRequest, json_width,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .route("/api/sessions/{id}", get(session_detail))
```

with:

```rust
        .route("/api/sessions/{id}", get(session_detail).patch(update_session))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        _ => format!("hat {hat_id}"),
    }
}
```

with:

```rust
        _ => format!("hat {hat_id}"),
    }
}

/// `PATCH /api/sessions/{id}` (ACP core §9): re-assign the session to another
/// hat (ACP core §4.9), 200 with its detail. Only with no running adapter,
/// and only from a session stepped up within five minutes (plan 5d decision
/// 2): it moves the session's history into another hat's reach.
async fn update_session(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<UpdateSessionRequest>,
) -> Response {
    let Some(hat_id) = req.hat_id else {
        return session_detail(State(state), Path(id)).await;
    };
    if !operator_session.stepped_up(unix_now()) {
        return hennery_kernel::auth::step_up_required();
    }
    match state.store.reassign_hat(&id, &hat_id) {
        Ok(Reassign::Done(event)) => {
            tracing::info!(session_id = %id, %hat_id, "session re-assigned");
            state.hub.publish(event);
        }
        Ok(Reassign::Unchanged) => {}
        Ok(Reassign::Attached(lifecycle)) => {
            return error(
                StatusCode::CONFLICT,
                &lifecycle,
                match lifecycle.as_str() {
                    "presumed_parked" => {
                        "its host has been away and may still run it: close the session first".to_string()
                    }
                    other => format!("the session is {other}: park or close it first"),
                },
            );
        }
        Ok(Reassign::UnknownHat) => return error(StatusCode::BAD_REQUEST, "invalid", "no such hat"),
        Ok(Reassign::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => return internal(err),
    }
    session_detail(State(state), Path(id)).await
}
```

Run: `cargo run -p hennery-proto --bin gen`

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test -p hennery-testkit --locked --test resolve --test reconcile --test step_up`
Expected: PASS.

- [ ] **Step 5: Revert-probes**

- In `update_session`, remove the step-up check. The step-up test fails on `PATCH /api/sessions/s-9`. Restore it.
- Check step-up before reading `hat_id`. The step-up test fails on the `{}` PATCH. Restore it.
- Replace `state.hub.publish(event)` with `drop(event)`. `a_reassigned_session_resumes_in_its_new_hat_once_the_rules_agree` fails waiting for the published event. Restore it.

- [ ] **Step 6: The full checks**

Expected: all pass; **882 tests**.

- [ ] **Step 7: Commit**

```bash
git add crates schema web
git commit -m "feat(sessions): PATCH /api/sessions/{id} re-assigns a session's hat, behind step-up"
```

## After this plan

**What the frontend must do (plan 4):**
- **Re-assign:** offer it on a parked, closed or failed session, with ACP core §4.9's warning that the agent's history came from the old hat.
  - On 409 `presumed_parked`, offer "close it first".
  - On 403, step up and retry.
- **After re-assigning:** remind the operator that a resume needs the rules to agree with the new hat; `hat_mismatch` links to the rules.

**Obligations this plan hands on:**
- **The gateway (plan 8):**
  - revoke a session's token when it is re-assigned, and when it closes, at once and without waiting for its host;
  - mint a token only from the hat read in the start or resume transition's own transaction;
  - the hats a host can obtain are its default, its rules' hats, and any hat a session was re-assigned to and then resumed in; a compromised host chooses among them.
- **Purge:** sessions re-assigned into a hat are that hat's, by `hat_id`.

**Spec amendments:**
- decision 1: ACP core §4.9 and §5.1: a presumed-parked session is refused (`presumed_parked`), so a re-assigned session is never attached to a host.
- decision 2: kernel §3.4: re-assigning a session needs step-up.

Then, in order:
- **(4) Frontend shell**
- **(8) Gateway**

---

_Generated with Claude AI — please review before distribution._
