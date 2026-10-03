# MCP gateway: sessions get their MCP servers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** hennery's own sessions get their hat's MCP servers (plan 8, sub-plan 8e).
- **Delivery.** Every start and resume decides what its session gets (umbrella §8.5), and gets it inside its own transaction: the hat's connections mounted on the host, through the gateway with a fresh session token, then the hat's local stdio servers for that host.
- **Revocation.** Every site that ends a session's right to MCP revokes its token in that transition's transaction, and ends what is open on the token once it commits.
- **Stdio servers** (gateway §3.4): one set per (host, hat), with the API pre-designed and security-reviewed in the lane's "API design, 8e and 8f" (lane L13).
- **Two preconditions to minting**, from the review of PR #99: an upstream's `Mcp-Session-Id` is bound to the token and connection that opened it, and a JSON answer has a deadline.
- **The purge lane's hand-offs** (lane L21): the token revoke in session delete, and the gateway's part of a hat's purge at its reserved slot.

**Architecture:**
- `hennery-gateway` defines `SessionMcp` (ACP core §1, amended): `servers_in`, `revoke_in`, `revoke_host_in`, `cut` and `purge_hat`. Its implementation `GatewayMcp` mints and revokes in the sessions store's own `rusqlite::Transaction` (lane L1). `hennery-sessions` depends on the gateway through that trait only; the store holds an `Arc<dyn SessionMcp>`, `NoSessionMcp` until the collector sets the real one.
- `revocation.rs`: a registry of open watches per token hash. The proxy watches a session token from before it resolves it until its answer's body ends; `Revocations::cut` cancels every watch on what a committed transaction invalidated.
- `stdio.rs`: the sets, their env values sealed per row (`crypto::seal_stdio`), the two routes `GET`/`PUT /api/mcp/stdio-servers?host_id=&hat_id=`.
- `session_id.rs`: `SessionIds`, an HMAC-SHA256 under a per-process key that wraps the upstream's `Mcp-Session-Id` going down and verifies and strips it going up.
- `hennery-proto`: the delivery decision `mcp_session_delivery` and the mapping `McpAgentDelivery::of`, which the host list and the decision share; the stdio DTOs; `HostItem.mcp_delivery`; `SessionDetail.mcp_delivery`.
- The kernel keeps a `hello`'s per-agent `mcp_isolation` on the host (migration after the hat logos'); the sessions store records each start's or resume's delivery (sessions migration 16).
- `hennery-sessions/src/redact.rs`: a session token an agent printed into an ACP payload is stored and published redacted.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite, axum 0.8, reqwest, tokio, tokio-util (`CancellationToken`), hmac 0.12.1 and sha2 (both already in the lock). No new crate in the lock.

**Spec:** gateway §3.1, §3.2, §3.4, §5.2, §5.5, §5.7; ACP core §1, §3, §4.8, §8, migrations; kernel §4, §5.5. All written back in Task 9. It builds on plans 8a–8d, plan 2026-10-15 "gateway differential" (#93) and "gateway JSON answers" (#99), plan 8c "host delivery", and the purge plans 9a–9d (#106, #107).

**Base:** `main` at `d14f556` (PR #112 merged). Anchors are taken from `d14f556`.

**Status:** written 2026-10-03, amended after the security review of 2026-10-03 (see "The security review's answers"). Not yet executed (see "Execution status").

**How the code blocks were made and checked:**
- The code was built first, TDD, on a scratch branch, and split into the task series below; every block was generated from that series' commits by a script, one region of a file per block.
- The plan was replayed from its own text onto `d14f556` in a scratch worktree, task by task; the replayed tree matched the series' byte for byte. `schema/hennery-protocol.schema.json` and `web/src/generated/protocol.ts` are not in blocks: Task 1 regenerates them. Nor is `Cargo.lock`: Tasks 3, 4 and 5 let cargo update it.
- 127 revert-probes were run; 126 fail as expected, and one is inert by construction (see "Revert-probes").

## Execution status

Not yet executed.

## Scope

1. The wire types: `McpSessionDeliveryMode` and the decision function; `McpAgentDelivery` on `HostItem`; `McpSessionDelivery` on `SessionDetail`; the stdio set's DTOs; the `mcp_stdio` capability (Task 1).
2. A `hello`'s per-agent MCP isolation kept on the host (Task 2).
3. The gateway: stdio sets and their routes; `SessionMcp` and `GatewayMcp`; the revocation registry and the proxy's watch; the purge's stdio part (Task 3).
4. The sessions module: delivery at every start and resume, revoke at every site of lane L4 (including delete, L21's hand-off 1), the redaction, the collector's wiring, and the end-to-end tests (Task 4).
5. The session id binding and the JSON answer deadline (Task 5).
6. Differential tests for the two new filters on parsed input (lane L16, Task 6).
7. A hat's purge runs the gateway's part first (L21's hand-off 2, Task 7).
8. The security review's amendments (Task 8).
9. The spec write-back (Task 9).

**Out:**
- OAuth (8f), standalone clients and renderers (8g), Codex's composed home (8h).
- `hats_with_credentials` counting stdio env values (lane L20): it is not on `main` yet.
- A timeline event for the delivery (E10's "may change"): only `SessionDetail.mcp_delivery`.

## Decisions this plan makes where the spec is silent

The API decisions E1–E11 are the lane's pre-designed API ("API design, implementation later: plans 8e and 8f", security-reviewed 2026-10-02, lane L13); this plan implements them unchanged:
- **E1** a stdio server's identity is its `name` within the set; **E2** names follow the slug rules, are unique in their set and disjoint from every connection slug of the owner, both ways (409 `slug_taken`, checked in the write's transaction); **E3** env values are write-only, a value left out keeps the stored one only from the same row and only while `command` is unchanged (R5, S1; else 400 `env_value_missing`); **E4** `command` and `args` are returned as stored; **E5** a revoked host's sets are read-only (409 `host_revoked`), a purge deletes the hat's sets; **E6** standalone mode has no stdio routes.
- **E7** per-agent isolation on `HostItem`, from the latest accepted `hello`, kept in the host registry; **E8** one mapping, `McpAgentDelivery::of`, read by the host list and by `mcp_session_delivery`; **E9** an unknown value reads as `default_hat_only`; **E10** each start and resume records what was decided on `SessionDetail.mcp_delivery`, a mode and a count, never a server, header or token; **E11** a host that never announced has no `mcp_delivery`.

This plan's own decisions:

1. **Mint and revoke inside the transitions' transactions** (lane L1). `servers_in(tx, session, mode)` reads the servers and mints the token in the start's or resume's transaction, so a failed mint rolls the start or resume back; `revoke_in` / `revoke_host_in` revoke in the transition's. A start whose mint fails stores no session; a resume whose mint fails leaves the session parked.
2. **The delivery decision is the sessions module's** (lane L2). It owns `sessions.hat_id`, computes `mixed` (a rule of the host naming another hat, or a `starting`, `active` or presumed-parked session of the host in another hat, this one included) and passes the mode in. The session counts itself, so a session outside the default hat always finds its host mixed: the arm "unmixed host, another hat" is unreachable and conservative (`fallback`, no servers). For the same reason `mixed` changes no outcome today; it is computed so that Codex's composed home (8h) can read it.
3. **The revoke sites** (lane L4): a host-reported park and close, an adapter exit, a failed start, a start or resume the route fails, closing an unattached session, a rejected reconcile close, reconcile's "never delivered" and "host restarted", a host revoke, a re-assignment, a delete (the purge lane's marked call site, L21 hand-off 1). Not revoked: a presumed park (ACP core §4.8), and a host's fact about a session that a newer resume has since superseded (the adapter exit guard checks the session is still attached).
4. **A resume supersedes**: its mint replaces the session's row, and the old token's watches are cut once the resume commits. A mode that does not deliver, or nothing mounted, revokes the old token.
5. **Only a host that announces `mcp_servers` is given servers** (mode `unsupported` otherwise), and `HostItem.mcp_delivery` is shown only for such a host.
6. **Refused for its servers**: a start or resume the host refuses as `mcp_isolation_unavailable` is marked failed and revoked; the refusal's message is answered redacted.
7. **The stdio env values are sealed per row**, bound to the row id, its host and its hat (`crypto::seal_stdio`, R5); they count as ciphertext for the master key's checks (`has_ciphertext`, `check_key`), so a wrong key stops the start as it does for credentials (8a decision 8).
8. **The binary's wiring is one function**, `gateway()`, which opens the gateway, gives the sessions store its `GatewayMcp` and returns the gateway's routes; a test checks the collector's store has it.
9. **The owner audit** (`owner_filter.rs`) reads `session.rs`, `stdio.rs` and the new statements of `tokens.rs` and the hosts registry.
10. **The host's frames in the log**: an invalid host frame is logged by its kind only, and a refusal's message is logged redacted; a test greps the collector's log for a token a host frame quoted.
11. **A token the agent printed is redacted** (the maintainer's open question Q2 of plan 8c; the lane parent's ruling (2) took its default, reversible, and it stays open with the operator). By shape: every run of `hnry_session_` in any case followed by at least 8 hexadecimal digits, in what is stored of an ACP payload and in what is published of it, questions and titles included. Not caught: a token split across two streamed updates, or encoded otherwise; it stops working at the session's next park. Two object keys of a payload that redact alike are merged into one (the security review's finding 5, accepted).
12. **A revoke ends what is open on the token** (the fleet parent's ruling of 2026-10-02, lane L17). The proxy watches a session token from before it is resolved until its answer's body ends (a `select!` around the head, and `cut_off` around the body, the JSON read included); every revoke kind, a supersession and a purge cut their tokens' watches after their commit. Cutting before the commit would race a rollback; a revoke that commits before the resolve makes it fail. Another token's streams survive. The sessions module cuts in one place, `store.rs`'s `commit_then_cut(tx, mcp, cut)`, which takes the transaction, commits it, then cuts; `tests/cut_after_commit.rs` reads the crate's sources and fails on any other `.cut(` (the security review's finding 1). A host revoke revokes and cuts its tokens at once (`Store::revoke_host_tokens`), before the route waits for the host's connection to close (finding 2); its later `revoke_host` finds none left. Scope: a token's revoke only. A change to a connection (unmount, delete, edit) is refused at request time, as gateway §3.2 has it; whether it should cut open streams too is the maintainer's (open question 1).
13. **The upstream's `Mcp-Session-Id` is bound to the token and connection** (the review of #99, finding 2). The id sent down is `<upstream id>.<tag>`, `tag` the 64 lowercase hex digits of an HMAC-SHA256 under a per-process random key over the token's hash, the connection's id and the upstream id, each length-prefixed. Going up, the id is split at its last `.`, verified and stripped; no `.`, a bad tag, another token's or connection's, or more than one header value is 404 `not_found` with nothing forwarded. An upstream answer with two ids is 502 `upstream_invalid`. The `Answerer` (refused server requests answered upstream) uses the bare id. No session table. Cost: after a restart every id is refused, and clients re-initialize (MCP's streamable HTTP allows it; an operator item).
14. **A JSON answer has a deadline**: `Limits::answer_timeout`, 60 s from the head (`ANSWER_TIMEOUT`), around reading it whole; past it, 502 `upstream_unreachable` and nothing goes down. The review of #99's finding 4 handed this to 8e or 8f. An event stream has none.
15. **A hat's purge runs the gateway's part first** (kernel §5.5's A15; L21 hand-off 2). The route and `on_hat_purged` owe the purge's checkpoint, then run `SessionMcp::purge_hat` (its tokens, stdio sets, connections with their credentials and mounts, in one transaction, their streams cut), then the sessions' part. The gateway's deletes are the purge's first, so the checkpoint is owed before them (9a's A8): a crash in them leaves it owed, and their rows leave no trace in the database files. A failed gateway part stops the purge before any session is deleted, and the hat stays frozen.

16. **`Last-Event-ID` goes up only beside a bound session id** (the security review's finding 4): an upstream that replays by event id alone would otherwise replay another session's stream to any token on the connection. Without a session id the cursor is dropped, and the request goes up without it.

## The security review's answers

**The security review (opus, security-auditor, 2026-10-03), on the maintainer's behalf: approve after amendments**, none blocking. It read the code at the series' tip in a scratch worktree and ran the gateway's `session_ids`, `revocation`, `proxy` and `stdio` suites, the sessions' `session_mcp` and the `redact` unit tests. It confirmed:
- decision 12: every site cuts after its commit, every proxy phase is watched (the head and the JSON read inside the `select!`, event streams through `cut_off`);
- decision 13: the MAC (32 random bytes per process, length-prefixed parts, constant-time verify), the parsing, the 404 on two values, only the bare id going up or to the `Answerer`;
- decision 14: the deadline wraps the whole read and fails closed;
- decision 15: the order and the checkpoint on every way out;
- decision 11, for its stated scope (it notes that streamed chunks are the common case for message text);
- decision 2 against umbrella §8.5, and probe 11 left inert;
- the stdio sets: the AAD, R5, S1, step-up, the namespace in one transaction, the limits, and no value or token in a log, event, SSE or `SessionDetail`.

| Finding | Taken how |
|---|---|
| 1 (should) Nothing tested the watch before the resolve, nor a cut after its commit | `a_revoke_committed_during_the_resolve_ends_the_request` (a `ClientIdentity` that commits a revoke inside the resolve; probe 118); `commit_then_cut` and the source audit `every_cut_goes_through_commit_then_cut` (probes 122, 123) |
| 2 (should) A host revoke cut its streams only after waiting for its connection | `Store::revoke_host_tokens` at once in the route; `a_host_revoke_cuts_its_streams_without_waiting_for_its_connection` (probes 124–126) |
| 3 (note) No revoke during a JSON read | `a_revoke_while_a_json_answer_is_read_ends_the_request` (probe 119) |
| 4 (note) `Last-Event-ID` went up unbound | Dropped without a bound session id (decision 16; probes 120, 121) |
| 5 (note) Two keys that redact alike merge | Accepted and documented (decision 11). The reviewer's placeholder on a failed re-read is declined: it showed the re-read cannot fail, so the code would be unreachable and unprobed |
| 6 (note) The plan named the wrong stdio route | Fixed |
| 7 (note) `NoSessionMcp::purge_hat` returns `Ok` | Declined: the purge tests of plan 9c run without a gateway and must keep passing; the collector's wiring test (`the_collector_gives_its_sessions_the_gateway`, probe 88) covers production |
| 8 (note) `mixed` changes no outcome | "After this plan": 8h owes a test and a probe for `mixed` once it can change one |

**Product questions the review raised**, neither answered by the specs nor decided here (both for the maintainer, neither blocking: gateway §3.2 and the fleet parent's ruling on revokes cover what 8e builds):
1. Should a change to a connection (unmount, delete, URL or credential change) cut the streams already open on it? Today it is refused at request time only.
2. Should stdio env values an agent echoes be redacted by value? A variant of Q2 (decision 11), which stays open with the operator.

**The scoped re-confirmation (a fresh opus reviewer, security-auditor, 2026-10-03), scoped to the amendments, the two declines and the product questions: confirmed with notes, none blocking.** It ran `cut_after_commit`, `revocation`, the replay-cursor test and the host-revoke test in the series' worktree. It found every taken item correct and biting, both declines justified, the product questions rightly non-blocking, and no new security issue from the amendments; dropping `Last-Event-ID` without a session id breaks no flow of an upstream that issues sessions (MCP requires the client to send the id), and costs only a session-less upstream its optional resumption. Its notes, all taken:
1. Probe 126 removed the cut with the revoke, so it duplicated 125: the host-revoke test now also asserts, while the route still waits, that the token's row is revoked and a fresh request is 404; probe 126 now keeps the cut and rolls the revoke back, and bites. (A fresh request is refused by the host's own revoke anyway: the proxy's scope joins on the host not being revoked.)
2. A failed early revoke stopped the route before its disconnect: now it is logged and the route goes on (the disconnect is the stronger control, and `on_host_revoked` revokes the tokens again).
3. The maintainer should decide open question 1 before the frontend offers deleting a connection or rotating its credential: "After this plan".
4. An upstream that ignores `Mcp-Session-Id` can still replay by event id alone: one line in gateway §5.2.

The host-revoke test is timing-dependent (the cut within 5 s, the route's wait 10 s): it passed with four copies of its binary in parallel.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`. After every task the five checks pass:
  - `nix develop -c cargo fmt --all --check`
  - `nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings`
  - `nix develop -c cargo clippy -p hennery --locked -- -D warnings`
  - `nix develop -c cargo test --workspace --locked`
  - `nix develop -c cargo run -p hennery-proto --bin gen -- --check`
- **Mint and revoke in the caller's transaction; cut only after its commit.** No revoke site may cut before `tx.commit()`.
- **Never a token in a log, an event, SSE or `SessionDetail`.** `mcp_delivery` is a mode and a count.
- Every SQL statement of the gateway and the registries names the owner (the owner audit).
- Every side-effect line has its own revert-probe, and every outcome of `McpSessionDeliveryMode` and `McpAgentDelivery` its own positive test.
- Tests that start the binary go through the testkit's scratch-home helper.
- Commits: Conventional Commits, the gmail identity (signing allowed since 2026-10-02; `git log --format='%ae' origin/main..HEAD` before every push).

## Review Focus

1. **Every revoke site revokes in its transaction and cuts after its commit.**
   - Tests: `crates/hennery-sessions/tests/session_mcp.rs`, one per site (decision 3), each asserting the row revoked, the token unresolvable and its watch cut; the negatives `a_presumed_park_does_not_revoke`, `a_stale_fact_does_not_revoke_a_resumes_fresh_token`.
2. **A revoke ends open streams** (decision 12): `a_revoke_cuts_the_tokens_open_stream_and_not_another_tokens`, `a_revoke_ends_a_request_not_yet_answered`, `a_finished_request_leaves_no_watch`, end to end `a_claude_session_gets_its_hats_servers_and_a_token_that_ends_with_its_park`.
3. **The delivery decision** (decision 2, E8): every mode in `crates/hennery-proto/tests/delivery.rs`, and per agent and hat in `session_mcp.rs`.
4. **A failed mint rolls back** (decision 1): `a_start_whose_mint_fails_stores_no_session`, `a_resume_whose_mint_fails_leaves_the_session_parked`, `a_rolled_back_mint_leaves_no_token_and_cuts_nothing`.
5. **Stdio sets** (E1–E6, decision 7): `crates/hennery-gateway/tests/stdio.rs`, the routes in `tests/api.rs`, the AAD binding.
6. **Session ids** (decision 13): `crates/hennery-gateway/tests/session_ids.rs`, the differential row `no_spelling_of_another_tokens_session_id_goes_up`. **The deadline** (decision 14): `a_json_answer_that_trickles_past_the_answer_timeout_is_502` (passes with four copies of the binary in parallel).
7. **The redaction** (decision 11): `a_token_in_an_acp_payload_is_stored_and_published_redacted`, `a_token_in_a_question_or_a_title_is_redacted_too`, the differential `no_decoder_reads_a_token_in_what_is_stored`, and the log canary `a_token_a_host_frame_quotes_never_reaches_the_collectors_log`.
8. **The purge** (decision 15): `crates/hennery-testkit/tests/purge.rs`'s three new tests.
9. **The review's amendments** (Task 8): the race and JSON-read revokes, the cut audit, the host revoke's immediate cut, the replay cursor.

## Revert-probes

Each line below was removed or neutered on the finished tree, the named test run, and the line restored. The script is the lane's ledger `probe.py`.

| # | Line | Test that failed |
|---|---|---|
| 0 | proxy body cut_off (`proxy.rs`) | `a_revoke_cuts_the_tokens_open_stream` |
| 1 | proxy head-phase select (`proxy.rs`) | `a_revoke_ends_a_request_not_yet_answered` |
| 2 | proxy body cut select (`proxy.rs`) | `a_revoke_cuts_the_tokens_open_stream` |
| 3 | proxy watch before resolve (`proxy.rs`) | `a_revoke_cuts_the_tokens_open_stream` |
| 4 | Revocations::cut cancels (`revocation.rs`) | `a_cut_cancels_every_watch` |
| 5 | Watch drop forgets (`revocation.rs`) | `the_last_watch_dropped_forgets` |
| 6 | GatewayMcp::cut (`session.rs`) | `a_host_reported_park_revokes` |
| 7 | GatewayMcp::purge_hat cut (`session.rs`) | `a_hat_purge_cuts_its_tokens` |
| 8 | servers_in supersession cut (`session.rs`) | `a_resume_supersedes_and_cuts_the_old_token` |
| 9 | servers_in revoke, mode withholds (`session.rs`) | `a_resume_supersedes_and_cuts_the_old_token` |
| 10 | servers_in revoke, nothing mounted (`session.rs`) | `a_resume_with_nothing_mounted_revokes` |
| 11 | deliver_in mixed (expected inert by construction) (`store.rs`) | `none (inert by construction)` |
| 12 | servers_in delivers check (`session.rs`) | `a_mode_that_does_not_deliver_mints_nothing` |
| 13 | mounted slugs: revoked host (`session.rs`) | `a_revoked_hosts_mounts_are_not_delivered` |
| 14 | mounted slugs: the hat (`session.rs`) | `a_session_gets_its_hats_mounted_connections` |
| 15 | stdio delivered (`session.rs`) | `a_session_gets_its_hats_mounted_connections` |
| 16 | revoke_in cut only live (`session.rs`) | `revoke_in_cuts_a_live_token_and_only_that` |
| 17 | revoke_host_in revokes (`session.rs`) | `revoke_host_in_cuts_every_live_token` |
| 18 | revoke_host_in cut (`session.rs`) | `revoke_host_in_cuts_every_live_token` |
| 19 | purge_hat takes tokens (`store.rs`) | `a_hat_purge_takes_its_tokens_and_then_the_hat_can_go` |
| 20 | purge_hat takes stdio (`store.rs`) | `a_hat_purge_takes_its_sets` |
| 21 | purge_hat names its cut (`store.rs`) | `a_hat_purge_cuts_its_tokens` |
| 22 | create refuses a stdio name (`store.rs`) | `names_and_slugs_are_one_namespace_both_ways` |
| 23 | has_ciphertext counts stdio (`store.rs`) | `stdio_values_count_as_ciphertext` |
| 24 | check_key opens stdio (`store.rs`) | `stdio_values_count_as_ciphertext` |
| 25 | stdio slug_taken (`stdio.rs`) | `names_and_slugs_are_one_namespace_both_ways` |
| 26 | stdio owner limit (`stdio.rs`) | `the_owner_has_at_most_1024` |
| 27 | stdio set limit (`stdio.rs`) | `the_stdio_routes_answer_their_codes` |
| 28 | stdio S1 command unchanged (`stdio.rs`) | `a_changed_command_must_resend_its_values` |
| 29 | stdio env missing (`stdio.rs`) | `a_value_never_stored_cannot_be_kept` |
| 30 | stdio host revoked (`stdio.rs`) | `an_unknown_host_or_hat_is_not_found_and_a_revoked_host` |
| 31 | stdio not found (`stdio.rs`) | `an_unknown_host_or_hat_is_not_found_and_a_revoked_host` |
| 32 | stdio frozen hat (`stdio.rs`) | `a_hat_being_purged_takes_no_set` |
| 33 | stdio drops left-out names (`stdio.rs`) | `a_set_is_stored_listed_and_replaced_whole` |
| 34 | stdio invalid name (`stdio.rs`) | `the_stdio_routes_answer_their_codes` |
| 35 | stdio AAD binds host (`crypto.rs`) | `a_stdio_blob_opens_only_for_its_row_host_and_hat` |
| 36 | api PUT step-up (`api.rs`) | `a_stdio_set_is_read_freely_and_replaced_behind_step_up` |
| 37 | api id bound (`api.rs`) | `the_stdio_routes_answer_their_codes` |
| 38 | deliver_in default hat (`store.rs`) | `codex_in_the_default_hat_is_unisolated` |
| 39 | deliver_in capable (`store.rs`) | `a_host_without_the_capability_gets_nothing` |
| 40 | deliver_in records (`store.rs`) | `claude_gets_its_hats_servers_isolated` |
| 41 | delete scrubs the record (`store.rs`) | `a_delete_revokes` |
| 42 | resume cut (`store.rs`) | `a_resume_supersedes_the_token_and_cuts` |
| 43 | mark_failed revoke (`store.rs`) | `a_start_the_route_fails_revokes` |
| 44 | mark_failed_if_starting revoke (`store.rs`) | `a_resume_the_route_fails_revokes` |
| 45 | close_in revoke (`store.rs`) | `closing_an_unattached_session_revokes` |
| 46 | close_now cut (`store.rs`) | `closing_an_unattached_session_revokes` |
| 47 | rejected reconcile close cut (`store.rs`) | `a_rejected_reconcile_close_revokes` |
| 48 | delete's own revoke (`store.rs`) | `a_delete_revokes_a_token_left_live_on_a_closed` |
| 49 | delete cut (`store.rs`) | `a_delete_revokes` |
| 50 | reassign revoke (`store.rs`) | `a_reassignment_revokes` |
| 51 | reassign cut (`store.rs`) | `a_reassignment_revokes` |
| 52 | revoke_host revoke (`store.rs`) | `a_host_revoke_revokes_every_token` |
| 53 | revoke_host cut (`store.rs`) | `a_host_revoke_revokes_every_token` |
| 54 | reconcile start_not_delivered revoke (`store.rs`) | `reconcile_revokes_a_start_the_host_never_got` |
| 55 | reconcile host_restarted revoke (`store.rs`) | `reconcile_revokes_a_session_a_restarted_host_lost` |
| 56 | reconcile cut (`store.rs`) | `reconcile_revokes_a_session_a_restarted_host_lost` |
| 57 | ingest start_failed revoke (`store.rs`) | `a_failed_start_revokes` |
| 58 | ingest session_parked revoke (`store.rs`) | `a_host_reported_park_revokes` |
| 59 | ingest session_closed revoke (`store.rs`) | `a_host_reported_close_revokes` |
| 60 | ingest adapter_exited revoke (`store.rs`) | `an_adapter_exit_revokes_ahead_of_its_park` |
| 61 | ingest adapter_exited attached guard (`store.rs`) | `a_stale_fact_does_not_revoke_a_resumes_fresh_token` |
| 62 | ingest cut (`store.rs`) | `a_host_reported_park_revokes` |
| 63 | ingest redacts (`store.rs`) | `a_token_in_an_acp_payload_is_stored_and_published_redacted` |
| 64 | purge hook gateway part (`lib.rs`) | `the_purge_hook_runs_the_gateways_part_too` |
| 65 | host_item capability filter (`hosts.rs`) | `a_hosts_delivery_is_shown_only_when_it_takes_servers` |
| 66 | ws records isolation (`ws.rs`) | `a_claude_session_gets_its_hats_servers` |
| 67 | ws invalid frame logged by kind (`ws.rs`) | `a_token_a_host_frame_quotes` |
| 68 | ws refusal redacted in log (`ws.rs`) | `a_token_a_host_frame_quotes` |
| 69 | api start not sent mark_failed (`api.rs`) | `a_start_refused_for_its_servers_fails` |
| 70 | api resume not sent mark_failed (`api.rs`) | `a_resume_refused_for_its_servers_fails` |
| 71 | api not sent reason (`api.rs`) | `a_start_refused_for_its_servers_fails` |
| 72 | api refusal redacted (start) (`api.rs`) | `a_refusals_message_is_answered_redacted` |
| 73 | api refusal redacted (resume) (`api.rs`) | `a_refusals_message_is_answered_redacted` |
| 74 | api start frame carries servers (`api.rs`) | `a_claude_session_gets_its_hats_servers` |
| 75 | api resume frame carries servers (`api.rs`) | `a_claude_session_gets_its_hats_servers` |
| 76 | api mcp_context capability (`api.rs`) | `a_claude_session_gets_its_hats_servers` |
| 77 | api detail record (`api.rs`) | `a_claude_session_gets_its_hats_servers` |
| 78 | McpGiven waived only unisolated (`store.rs`) | `claude_gets_its_hats_servers_isolated` |
| 79 | kernel isolation bounded count (`hosts.rs`) | `a_hellos_mcp_isolation_is_bounded` |
| 80 | kernel isolation bounded ids (`hosts.rs`) | `a_hellos_mcp_isolation_is_bounded` |
| 81 | proto delivery: capable (`rest.rs`) | `a_host_without_the_capability` |
| 82 | proto delivery: unisolated (`rest.rs`) | `the_default_hat_is_unisolated` |
| 83 | proto delivery: fallback (`rest.rs`) | `another_hat_on_a_mixed_host_falls_back` |
| 84 | proto delivery: unmixed other hat (`rest.rs`) | `another_hat_on_an_unmixed_host` |
| 85 | proto delivery: isolated (`rest.rs`) | `an_isolated_agent_gets` |
| 86 | proto agent delivery: claude (`rest.rs`) | `claude_strict_is_isolated` |
| 87 | proto agent delivery: none (`rest.rs`) | `no_isolation_is_default_hat_only` |
| 88 | binary wires the gateway (`main.rs`) | `the_collector_gives_its_sessions_the_gateway` |
| 89 | sid: tag verified (`session_id.rs`) | `another_tokens_session_id_is_404_and_nothing_goes_up` |
| 90 | sid: lowercase hex only (`session_id.rs`) | `a_bare_forged_or_doubled_session_id_is_404` |
| 91 | sid: parts length-prefixed (`session_id.rs`) | `the_parts_cannot_be_shifted` |
| 92 | sid: MAC binds the connection (`session_id.rs`) | `a_session_id_from_another_connection_is_404` |
| 93 | sid: MAC binds the token (`session_id.rs`) | `another_tokens_session_id_is_404_and_nothing_goes_up` |
| 94 | sid: key random per process (`session_id.rs`) | `a_restarted_proxy_refuses_the_ids_it_gave_before` |
| 95 | proxy: unbound id is 404 (`proxy.rs`) | `another_tokens_session_id_is_404_and_nothing_goes_up` |
| 96 | proxy: two ids are 404 (`proxy.rs`) | `a_bare_forged_or_doubled_session_id_is_404` |
| 97 | proxy: unwrapped id goes up (`proxy.rs`) | `a_session_id_comes_down_wrapped_and_goes_up_bare` |
| 98 | proxy: answer's id wrapped (`proxy.rs`) | `a_session_id_comes_down_wrapped_and_goes_up_bare` |
| 99 | proxy: two answered ids 502 (`proxy.rs`) | `an_answer_with_two_session_ids_is_502` |
| 100 | proxy: Answerer gets the bare id (`proxy.rs`) | `a_refused_server_request_s_answer_takes_the_request_s_session_and_a_permit` |
| 101 | proxy: JSON answer deadline (`proxy.rs`) | `a_json_answer_that_trickles_past_the_answer_timeout_is_502` |
| 102 | stdio input deny_unknown: request (`rest.rs`) | `every_decoder_reads_an_accepted_stdio_set_as_it_was_stored` |
| 103 | stdio input deny_unknown: server (`rest.rs`) | `every_decoder_reads_an_accepted_stdio_set_as_it_was_stored` |
| 104 | stdio input deny_unknown: env (`rest.rs`) | `every_decoder_reads_an_accepted_stdio_set_as_it_was_stored` |
| 105 | stdio command control chars (`stdio.rs`) | `every_decoder_reads_an_accepted_stdio_set_as_it_was_stored` |
| 106 | stdio args NUL (`stdio.rs`) | `every_decoder_reads_an_accepted_stdio_set_as_it_was_stored` |
| 107 | stdio env value NUL (`stdio.rs`) | `every_decoder_reads_an_accepted_stdio_set_as_it_was_stored` |
| 108 | redact keys (`redact.rs`) | `no_decoder_reads_a_token_in_what_is_stored` |
| 109 | redact any case (`redact.rs`) | `no_decoder_reads_a_token_in_what_is_stored` |
| 110 | route gateway part (`hats.rs`) | `a_purge_takes_the_hats_gateway_rows_and_leaves_another_hats` |
| 111 | route stops on gateway error (`hats.rs`) | `a_failed_gateway_part_stops_a_purge_before_its_sessions` |
| 112 | route order gateway first (`hats.rs`) | `a_failed_gateway_part_stops_a_purge_before_its_sessions` |
| 113 | hook stops on gateway error (`lib.rs`) | `a_failed_gateway_part_stops_a_purge_before_its_sessions` |
| 114 | hook order gateway first (`lib.rs`) | `a_failed_gateway_part_stops_a_purge_before_its_sessions` |
| 115 | route owes before gateway part (`hats.rs`) | `the_purges_checkpoint_is_owed_before_the_gateways_part` |
| 116 | route owe after gateway part (order) (`hats.rs`) | `the_purges_checkpoint_is_owed_before_the_gateways_part` |
| 117 | hook owe after gateway part (order) (`lib.rs`) | `the_purges_checkpoint_is_owed_before_the_gateways_part` |
| 118 | proxy watch before resolve (order) (`proxy.rs`) | `a_revoke_committed_during_the_resolve_ends_the_request` |
| 119 | proxy head-phase select (JSON read) (`proxy.rs`) | `a_revoke_while_a_json_answer_is_read_ends_the_request` |
| 120 | Last-Event-ID not forwarded on its own (`proxy.rs`) | `a_replay_cursor_goes_up_only_with_a_bound_session_id` |
| 121 | Last-Event-ID beside a bound session id (`proxy.rs`) | `a_replay_cursor_goes_up_only_with_a_bound_session_id` |
| 122 | commit_then_cut commits first (`store.rs`) | `every_cut_goes_through_commit_then_cut` |
| 123 | a site cutting before its commit (`store.rs`) | `every_cut_goes_through_commit_then_cut` |
| 124 | host revoke route: tokens at once (`hosts.rs`) | `a_host_revoke_cuts_its_streams_without_waiting_for_its_connection` |
| 125 | revoke_host_tokens cuts (`store.rs`) | `a_host_revoke_cuts_its_streams_without_waiting_for_its_connection` |
| 126 | revoke_host_tokens revokes (cut, not revoked) (`store.rs`) | `a_host_revoke_cuts_its_streams_without_waiting_for_its_connection` |

**Probe 11** (`mixed` forced to `true`) is inert by construction (decision 2): `mixed` changes no outcome today. The security review accepted it (finding 8). Probes 0–117 were run on the series' Task 7 tree, 118–126 on Task 8's; the anchors of the ten cut sites were rewritten for `commit_then_cut` and those probes run again.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-proto/src/rest.rs`, `codegen.rs`; `tests/delivery.rs` | delivery modes, the decision, stdio DTOs, `HostItem`/`SessionDetail` fields | 1 |
| `crates/hennery-kernel/src/hosts.rs`, `schema.rs` | `hosts.mcp_isolation` | 2 |
| `crates/hennery-gateway/src/stdio.rs` (new), `revocation.rs` (new), `session.rs` (new) | stdio sets; the watch registry; `SessionMcp`/`GatewayMcp` | 3 |
| `crates/hennery-gateway/src/proxy.rs`, `store.rs`, `tokens.rs`, `crypto.rs`, `api.rs`, `schema.rs` | the watch and cut; purge's stdio part; stdio sealing; routes; gateway migration 3 | 3 |
| `crates/hennery-kernel/src/capabilities.rs` | `mcp_stdio` on | 3 |
| `crates/hennery-sessions/src/store.rs`, `api.rs`, `hosts.rs`, `ws.rs`, `lib.rs`, `redact.rs` (new) | delivery, revoke sites, redaction, migration 16 | 4 |
| `crates/hennery/src/main.rs` | `gateway()` wiring | 3, 4 |
| `crates/hennery-testkit/tests/session_gateway.rs`, `session_gateway_log.rs` (new) | end to end, log canary | 4 |
| `crates/hennery-gateway/src/session_id.rs` (new) | `SessionIds` | 5 |
| `crates/hennery-gateway/tests/support/differential.rs`, `crates/hennery-sessions/src/redact.rs` | L16 differential tests | 6 |
| `crates/hennery-sessions/src/hats.rs`, `lib.rs`; `crates/hennery-testkit/tests/purge.rs` | the purge's order | 7 |
| `crates/hennery-sessions/tests/cut_after_commit.rs` (new); `store.rs`'s `commit_then_cut`, `revoke_host_tokens`; `hosts.rs`'s revoke route; `proxy.rs`'s `LAST_EVENT_ID` | the review's amendments | 8 |
| `docs/specs/…` | write-back | 9 |

**Reading the steps:** "Create `path`" is a whole file; "In `path`, replace: … with: …" replaces its one occurrence, in order, as the file stands after the blocks before it.

---
### Task 1: The wire types: delivery modes, the decision, the stdio DTOs

**Files:** Create `crates/hennery-proto/tests/delivery.rs`; modify `crates/hennery-proto/src/rest.rs`, `src/codegen.rs`, `tests/codegen.rs`, `tests/frames.rs`; `crates/hennery-sessions/src/api.rs` and `src/hosts.rs` (the new fields, `None` until Task 4); regenerate `schema/hennery-protocol.schema.json` and `web/src/generated/protocol.ts`.
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-proto/tests/codegen.rs`, replace:

  ```rust
  const GATEWAY_TYPES: [&str; 7] = [
  ```

with:

  ```rust
  const GATEWAY_TYPES: [&str; 16] = [
  ```

In `crates/hennery-proto/tests/codegen.rs`, replace:

  ```rust
      "McpCredentialRequest",
  ];
  ```

with:

  ```rust
      "McpCredentialRequest",
      // Plan 8e.
      "McpAgentDelivery",
      "McpSessionDeliveryMode",
      "McpSessionDelivery",
      "McpStdioEnvItem",
      "McpStdioServerItem",
      "McpStdioServerSet",
      "McpStdioEnvInput",
      "McpStdioServerInput",
      "McpStdioServersRequest",
  ];
  ```

In `crates/hennery-proto/tests/codegen.rs`, replace:

  ```rust
          "wrong_cred_kind",
          "internal",
  ```

with:

  ```rust
          "wrong_cred_kind",
          "host_revoked",
          "env_value_missing",
          "too_many_stdio_servers",
          "internal",
  ```

Create `crates/hennery-proto/tests/delivery.rs`:

  ```rust
  //! Plan 8e: the one mapping from a host's isolation to what a session gets
  //! (decision E8), each outcome on its own, and the stdio servers' wire
  //! types, whose `Debug`s never show an argument or a value.

  use hennery_proto::frames::McpIsolation;
  use hennery_proto::rest::{
      HostItem, McpAgentDelivery, McpSessionDelivery, McpSessionDeliveryMode, McpStdioEnvInput, McpStdioEnvItem,
      McpStdioServerInput, McpStdioServerItem, McpStdioServersRequest, mcp_session_delivery,
  };
  use serde_json::json;

  #[test]
  fn claude_strict_is_isolated() {
      assert_eq!(
          McpAgentDelivery::of(McpIsolation::ClaudeStrict),
          McpAgentDelivery::Isolated
      );
  }

  #[test]
  fn no_isolation_is_default_hat_only() {
      assert_eq!(
          McpAgentDelivery::of(McpIsolation::None),
          McpAgentDelivery::DefaultHatOnly
      );
  }

  #[test]
  fn a_host_without_the_capability_gets_nothing_whatever_it_isolates() {
      for isolation in [McpIsolation::ClaudeStrict, McpIsolation::None] {
          for (mixed, default) in [(false, false), (false, true), (true, false), (true, true)] {
              assert_eq!(
                  mcp_session_delivery(false, isolation, mixed, default),
                  McpSessionDeliveryMode::Unsupported
              );
          }
      }
  }

  #[test]
  fn an_isolated_agent_gets_its_hats_servers_in_every_hat() {
      for (mixed, default) in [(false, false), (false, true), (true, false), (true, true)] {
          assert_eq!(
              mcp_session_delivery(true, McpIsolation::ClaudeStrict, mixed, default),
              McpSessionDeliveryMode::Isolated
          );
      }
  }

  /// The default hat of a host that cannot isolate the agent: waived, mixed
  /// (the fallback's default hat) or not (a single-hat host).
  #[test]
  fn the_default_hat_is_unisolated_on_a_host_that_cannot_isolate() {
      for mixed in [false, true] {
          assert_eq!(
              mcp_session_delivery(true, McpIsolation::None, mixed, true),
              McpSessionDeliveryMode::Unisolated
          );
      }
  }

  #[test]
  fn another_hat_on_a_mixed_host_falls_back() {
      assert_eq!(
          mcp_session_delivery(true, McpIsolation::None, true, false),
          McpSessionDeliveryMode::Fallback
      );
  }

  /// Unreachable from the store (the session counts itself), and conservative.
  #[test]
  fn another_hat_on_an_unmixed_host_gets_nothing_too() {
      assert_eq!(
          mcp_session_delivery(true, McpIsolation::None, false, false),
          McpSessionDeliveryMode::Fallback
      );
  }

  #[test]
  fn only_isolated_and_unisolated_deliver() {
      let delivers: Vec<bool> = [
          McpSessionDeliveryMode::Isolated,
          McpSessionDeliveryMode::Unisolated,
          McpSessionDeliveryMode::Fallback,
          McpSessionDeliveryMode::Unsupported,
      ]
      .map(McpSessionDeliveryMode::delivers)
      .into();
      assert_eq!(delivers, [true, true, false, false]);
  }

  #[test]
  fn a_mode_reads_back_as_written_and_as_on_the_wire() {
      for mode in [
          McpSessionDeliveryMode::Isolated,
          McpSessionDeliveryMode::Unisolated,
          McpSessionDeliveryMode::Fallback,
          McpSessionDeliveryMode::Unsupported,
      ] {
          assert_eq!(McpSessionDeliveryMode::parse(mode.as_str()), Some(mode));
          assert_eq!(serde_json::to_value(mode).unwrap(), json!(mode.as_str()));
      }
      assert_eq!(McpSessionDeliveryMode::parse("everything"), None);
  }

  #[test]
  fn a_session_delivery_is_a_mode_a_count_and_a_time() {
      let delivery = McpSessionDelivery {
          mode: McpSessionDeliveryMode::Fallback,
          servers: 0,
          at: "2026-10-16T12:00:00Z".into(),
      };
      assert_eq!(
          serde_json::to_value(&delivery).unwrap(),
          json!({"mode": "fallback", "servers": 0, "at": "2026-10-16T12:00:00Z"})
      );
  }

  #[test]
  fn a_host_without_a_recorded_delivery_leaves_it_out() {
      let item = HostItem {
          host_id: "h".into(),
          name: "h".into(),
          platform: "linux".into(),
          host_version: "1".into(),
          capabilities: Default::default(),
          default_hat_id: "hat-1".into(),
          workspace_roots: vec![],
          connected: false,
          created_at: "2026-10-16T12:00:00Z".into(),
          last_seen_at: None,
          revoked_at: None,
          mcp_delivery: None,
      };
      let value = serde_json::to_value(&item).unwrap();
      assert!(value.get("mcp_delivery").is_none(), "{value}");
      let item = HostItem {
          mcp_delivery: Some([("claude".to_string(), McpAgentDelivery::Isolated)].into()),
          ..item
      };
      assert_eq!(
          serde_json::to_value(&item).unwrap()["mcp_delivery"],
          json!({"claude": "isolated"})
      );
  }

  const ARG: &str = "--key=arg-secret-0123";
  const VALUE: &str = "env-secret-0123";

  #[test]
  fn stdio_debugs_show_no_argument_and_no_value() {
      let input = McpStdioServersRequest {
          servers: vec![McpStdioServerInput {
              name: "files".into(),
              command: "files-mcp".into(),
              args: vec![ARG.into()],
              env: vec![McpStdioEnvInput {
                  name: "KEY".into(),
                  value: Some(VALUE.into()),
              }],
          }],
      };
      let item = McpStdioServerItem {
          name: "files".into(),
          command: "files-mcp".into(),
          args: vec![ARG.into()],
          env: vec![McpStdioEnvItem {
              name: "KEY".into(),
              has_value: true,
          }],
          created_at: String::new(),
          updated_at: String::new(),
      };
      let env = McpStdioEnvInput {
          name: "KEY".into(),
          value: Some(VALUE.into()),
      };
      for shown in [
          format!("{input:?} {input:#?}"),
          format!("{item:?} {item:#?}"),
          format!("{env:?}"),
      ] {
          assert!(!shown.contains(ARG) && !shown.contains(VALUE), "{shown}");
          assert!(shown.contains("files") || shown.contains("KEY"), "{shown}");
      }
  }

  #[test]
  fn a_put_refuses_an_unknown_field_and_keeps_an_absent_or_null_value() {
      let body = json!({"servers": [{"name": "files", "command": "c", "env": [{"name": "A"}, {"name": "B", "value": null}, {"name": "C", "value": ""}]}]});
      let parsed: McpStdioServersRequest = serde_json::from_value(body).unwrap();
      let values: Vec<Option<&str>> = parsed.servers[0].env.iter().map(|e| e.value.as_deref()).collect();
      assert_eq!(values, [None, None, Some("")]);
      assert!(parsed.servers[0].args.is_empty());
      let unknown = json!({"servers": [{"name": "files", "command": "c", "cwd": "/"}]});
      assert!(serde_json::from_value::<McpStdioServersRequest>(unknown).is_err());
  }
  ```

In `crates/hennery-proto/tests/frames.rs`, replace:

  ```rust
          pending: vec![],
      };
  ```

with:

  ```rust
          pending: vec![],
          mcp_delivery: None,
      };
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test -p hennery-proto --locked
   ```

   It does not compile: `McpSessionDeliveryMode`, `mcp_session_delivery`, `McpAgentDelivery`, `McpSessionDelivery` and the stdio DTOs do not exist.

   ```sh
   git add -A && git commit -m "test(proto): what a host and a session are told of their MCP delivery, and the stdio sets"
   ```

- [ ] **Step 3: Implement**

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::DeploymentMode,
          rest::CapabilitiesResponse,
      );
      // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
  ```

with:

  ```rust
          rest::DeploymentMode,
          rest::CapabilitiesResponse,
          rest::McpAgentDelivery,
          rest::McpSessionDeliveryMode,
          rest::McpSessionDelivery,
          rest::McpStdioEnvItem,
          rest::McpStdioServerItem,
          rest::McpStdioServerSet,
          rest::McpStdioEnvInput,
          rest::McpStdioServerInput,
          rest::McpStdioServersRequest,
      );
      // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
  ```

In `crates/hennery-proto/src/codegen.rs`, replace:

  ```rust
          rest::CapabilitiesResponse,
      );
  ```

with:

  ```rust
          rest::CapabilitiesResponse,
          rest::McpAgentDelivery,
          rest::McpSessionDeliveryMode,
          rest::McpSessionDelivery,
          rest::McpStdioEnvItem,
          rest::McpStdioServerItem,
          rest::McpStdioServerSet,
          rest::McpStdioEnvInput,
          rest::McpStdioServerInput,
          rest::McpStdioServersRequest,
      );
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      pub pending: Vec<PendingItem>,
  }
  ```

with:

  ```rust
      pub pending: Vec<PendingItem>,
      /// What its latest start or resume was given (plan 8e decision E10);
      /// absent for a session not started or resumed since plan 8e.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "McpSessionDelivery | undefined", optional)]
      pub mcp_delivery: Option<McpSessionDelivery>,
  }
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      pub revoked_at: Option<String>,
  }
  ```

with:

  ```rust
      pub revoked_at: Option<String>,
      /// Per agent id, which hats' sessions get gateway MCP servers, from the
      /// `mcp_isolation` of its latest accepted `hello` (plan 8e). Absent: no
      /// such `hello` was recorded yet. Only for a host whose `capabilities`
      /// include `mcp_servers`; one without receives no servers at all.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "Record<string, McpAgentDelivery> | undefined", optional)]
      pub mcp_delivery: Option<std::collections::BTreeMap<String, McpAgentDelivery>>,
  }
  ```

In `crates/hennery-proto/src/rest.rs`, replace:

  ```rust
      pub features: Vec<String>,
  }
  ```

with:

  ```rust
      pub features: Vec<String>,
  }

  /// Which hats' sessions on a host get gateway MCP servers for one agent
  /// (umbrella §8.5, plan 8e), from the agent's isolation as the host last
  /// announced it. Read leniently by the UI: a value it does not know is to be
  /// shown as `default_hat_only`.
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum McpAgentDelivery {
      /// The host keeps the agent's sessions to the servers hennery passes:
      /// every hat's sessions get their own hat's servers.
      Isolated,
      /// The host cannot isolate this agent: sessions in the host's default hat
      /// get the default hat's servers (and also load the user's own MCP
      /// configuration); sessions in any other hat get none (the fallback).
      DefaultHatOnly,
  }

  impl McpAgentDelivery {
      /// The one mapping (plan 8e decision E8), which the host list and the
      /// delivery decision (`mcp_session_delivery`) both read.
      pub fn of(isolation: crate::frames::McpIsolation) -> Self {
          match isolation {
              crate::frames::McpIsolation::ClaudeStrict => Self::Isolated,
              crate::frames::McpIsolation::None => Self::DefaultHatOnly,
          }
      }
  }

  /// What a session's latest start or resume was given (plan 8e decision
  /// E10).
  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(rename_all = "snake_case")]
  pub enum McpSessionDeliveryMode {
      /// Its hat's servers, the agent kept to them.
      Isolated,
      /// Its hat's servers, isolation waived: the host's default hat on a host
      /// that cannot isolate the agent. It also loads the user's own MCP
      /// configuration.
      Unisolated,
      /// None: another hat than the host's default, on a host that cannot
      /// isolate the agent (umbrella §8.5).
      Fallback,
      /// None: the host cannot receive MCP servers (no `mcp_servers`
      /// capability).
      Unsupported,
  }

  impl McpSessionDeliveryMode {
      /// Whether the session gets its hat's servers (and a gateway token).
      pub fn delivers(self) -> bool {
          matches!(self, Self::Isolated | Self::Unisolated)
      }

      /// The column value (`sessions.mcp_delivery_mode`), as on the wire.
      pub fn as_str(self) -> &'static str {
          match self {
              Self::Isolated => "isolated",
              Self::Unisolated => "unisolated",
              Self::Fallback => "fallback",
              Self::Unsupported => "unsupported",
          }
      }

      /// The mode a column value names; `None` for anything else.
      pub fn parse(text: &str) -> Option<Self> {
          [Self::Isolated, Self::Unisolated, Self::Fallback, Self::Unsupported]
              .into_iter()
              .find(|mode| mode.as_str() == text)
      }
  }

  /// The delivery decision for one start or resume (umbrella §8.5; plan 8e
  /// decision E8, the gateway lane's L2): `capable`, the host's connection
  /// announced `mcp_servers`; `isolation`, how it isolates the session's
  /// agent; `mixed`, a rule of the host names another hat than its default,
  /// or a `starting`, `active` or presumed-parked session on it (this one
  /// included) is in another hat; `is_default_hat`, the session is in the
  /// host's default hat.
  ///
  /// The session counts itself, so a session outside the default hat always
  /// finds its host mixed: the arm for an unmixed host and another hat is
  /// unreachable, and is the conservative one (no servers).
  pub fn mcp_session_delivery(
      capable: bool,
      isolation: crate::frames::McpIsolation,
      mixed: bool,
      is_default_hat: bool,
  ) -> McpSessionDeliveryMode {
      if !capable {
          return McpSessionDeliveryMode::Unsupported;
      }
      match (McpAgentDelivery::of(isolation), mixed, is_default_hat) {
          (McpAgentDelivery::Isolated, _, _) => McpSessionDeliveryMode::Isolated,
          // A mixed host's fallback keeps the default hat's servers; an
          // unmixed host is a single-hat host. Either way, waived.
          (McpAgentDelivery::DefaultHatOnly, _, true) => McpSessionDeliveryMode::Unisolated,
          (McpAgentDelivery::DefaultHatOnly, true, false) => McpSessionDeliveryMode::Fallback,
          // Unreachable (see above): conservative.
          (McpAgentDelivery::DefaultHatOnly, false, false) => McpSessionDeliveryMode::Fallback,
      }
  }

  /// On `SessionDetail` (plan 8e decision E10): never a server, a header or a
  /// token, only the mode and a count.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct McpSessionDelivery {
      /// What it was given.
      pub mode: McpSessionDeliveryMode,
      /// How many servers it was given (connections plus stdio servers): `0`
      /// with `isolated` means its hat has none on this host.
      #[ts(type = "number")]
      pub servers: u32,
      /// RFC 3339: the start or resume it describes.
      pub at: String,
  }

  /// One environment variable of a stdio server, as `GET` answers it: its
  /// name and whether a value is stored. The value is sealed at rest
  /// (gateway spec §6) and no route answers it.
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct McpStdioEnvItem {
      /// `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
      pub name: String,
      /// A value is stored (possibly `""`).
      pub has_value: bool,
  }

  /// One local stdio server of a (host, hat) (gateway spec §3.4): passed to
  /// that hat's sessions on that host as an ACP stdio `mcpServers` entry
  /// named `hennery-<name>`. The agent runs it on the host; the gateway does
  /// not proxy it. Its `Debug` shows the name, the command and how many
  /// args, never the args.
  #[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct McpStdioServerItem {
      /// `^[a-z0-9][a-z0-9-]{0,47}$`, unique in its set, and never one of the
      /// owner's connection slugs.
      pub name: String,
      /// As stored: run by the agent, found on its `PATH` unless absolute.
      pub command: String,
      /// As stored. Not sealed: a secret belongs in `env`.
      pub args: Vec<String>,
      /// Names only, in the order given.
      pub env: Vec<McpStdioEnvItem>,
      /// RFC 3339.
      pub created_at: String,
      /// RFC 3339: the `PUT` that last changed it.
      pub updated_at: String,
  }

  impl std::fmt::Debug for McpStdioServerItem {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("McpStdioServerItem")
              .field("name", &self.name)
              .field("command", &self.command)
              .field("args", &format_args!("<{} redacted>", self.args.len()))
              .field("env", &self.env)
              .finish_non_exhaustive()
      }
  }

  /// `GET /api/mcp/stdio-servers?host_id=&hat_id=` (200), and the answer to
  /// its `PUT` (200): one (host, hat)'s whole set, oldest first. A host or
  /// hat with none answers `servers: []`.
  ///
  /// Its own codes, beyond every route's (see `McpConnectionItem`): 400
  /// `invalid` (`host_id` or `hat_id` missing or over 64 bytes, never quoted
  /// back); 404 `not_found` (not one of the owner's hosts or hats; a revoked
  /// host is found).
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  pub struct McpStdioServerSet {
      /// The host whose agents run them.
      pub host_id: String,
      /// The hat whose sessions get them.
      pub hat_id: String,
      /// Oldest first.
      pub servers: Vec<McpStdioServerItem>,
  }

  /// One environment variable in a `PUT`: `value` absent (or `null`) keeps
  /// the value stored for this server name and variable name in the same
  /// (host, hat), and only while the server's `command` is unchanged; a
  /// string sets it. A variable left out of the list is deleted. Its `Debug`
  /// never shows the value.
  #[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(deny_unknown_fields)]
  pub struct McpStdioEnvInput {
      /// `^[A-Za-z_][A-Za-z0-9_]{0,127}$`, unique per server.
      pub name: String,
      /// Absent or `null`: kept. A string (`""` too): set; at most 8192
      /// bytes, no NUL.
      #[serde(default, skip_serializing_if = "Option::is_none")]
      #[ts(type = "string | undefined", optional)]
      pub value: Option<String>,
  }

  impl std::fmt::Debug for McpStdioEnvInput {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("McpStdioEnvInput")
              .field("name", &self.name)
              .field("value", &self.value.as_ref().map(|_| "<redacted>"))
              .finish()
      }
  }

  /// One server in a `PUT`. Its `Debug` shows the name and the command only.
  #[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(deny_unknown_fields)]
  pub struct McpStdioServerInput {
      /// `^[a-z0-9][a-z0-9-]{0,47}$`, unique in the set, and not one of the
      /// owner's connection slugs.
      pub name: String,
      /// 1 to 1024 bytes, no control characters.
      pub command: String,
      /// Absent: none. At most 64, each at most 4096 bytes without NUL, 16
      /// KiB in all.
      #[serde(default)]
      #[ts(type = "string[] | undefined", optional)]
      pub args: Vec<String>,
      /// Absent: none. At most 64.
      #[serde(default)]
      #[ts(type = "McpStdioEnvInput[] | undefined", optional)]
      pub env: Vec<McpStdioEnvInput>,
  }

  impl std::fmt::Debug for McpStdioServerInput {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("McpStdioServerInput")
              .field("name", &self.name)
              .field("command", &self.command)
              .finish_non_exhaustive()
      }
  }

  /// `PUT /api/mcp/stdio-servers?host_id=&hat_id=` (step-up): the (host,
  /// hat)'s whole set, replacing the one before, never a delta. `[]` deletes
  /// them all. 200 with the stored `McpStdioServerSet`. Applies to the next
  /// start or resume of a session of that hat on that host.
  ///
  /// Its own codes, beyond every route's (see `McpConnectionItem`): 403
  /// `step_up_required`; 404 `not_found` (host or hat); 409 `host_revoked`;
  /// 400 `invalid` (`message` names the server and the field; a value is
  /// never quoted back); 400 `env_value_missing` (a kept value that is not
  /// stored, or whose server's `command` changed); 409 `slug_taken` (a name
  /// one of the owner's connections has); 409 `too_many_stdio_servers` (more
  /// than 32 in the set, or 1024 for the owner). A refused set changes
  /// nothing.
  #[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
  #[serde(deny_unknown_fields)]
  pub struct McpStdioServersRequest {
      /// The whole set, in the order to keep for new servers.
      pub servers: Vec<McpStdioServerInput>,
  }

  impl std::fmt::Debug for McpStdioServersRequest {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("McpStdioServersRequest")
              .field("servers", &self.servers)
              .finish()
      }
  }
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      Json(SessionDetail {
          session: item,
  ```

with:

  ```rust
      Json(SessionDetail {
          mcp_delivery: None,
          session: item,
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
      HostItem {
          connected: state.hub.is_ready(&record.id),
  ```

with:

  ```rust
      HostItem {
          mcp_delivery: None,
          connected: state.hub.is_ready(&record.id),
  ```

- [ ] **Step 4: Run the checks**

   Regenerate the wire files, then run the five checks:

   ```sh
   nix develop -c cargo run -p hennery-proto --bin gen
   ```

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "feat(proto): MCP delivery for hosts and sessions, and the stdio set's wire types"
   ```

### Task 2: A hello's per-agent MCP isolation, kept on the host

**Files:** Modify `crates/hennery-kernel/src/hosts.rs`, `src/schema.rs`, `tests/hosts.rs`, `tests/owner.rs`; `crates/hennery-testkit/tests/owner_filter.rs` (the hosts registry's statement count).
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-kernel/tests/hosts.rs`, replace:

  ```rust
              .enroll(&later.code, &enrollment(&key(2)), NOW + PAIRING_CODE_TTL_SECS + 2)
              .unwrap(),
      );
  }
  ```

with:

  ```rust
              .enroll(&later.code, &enrollment(&key(2)), NOW + PAIRING_CODE_TTL_SECS + 2)
              .unwrap(),
      );
  }

  /// Plan 8e decision E7: a `hello`'s per-agent MCP isolation is kept for the
  /// host list, as of the latest one; none before the first.
  #[test]
  fn a_hellos_mcp_isolation_is_kept_as_of_the_latest() {
      use hennery_proto::frames::{AgentIsolation, McpIsolation};
      let hosts = Hosts::open_in_memory().unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      assert_eq!(hosts.host("host-1").unwrap().unwrap().mcp_isolation, None);
      let first = AgentIsolation(
          [
              ("claude".to_string(), McpIsolation::ClaudeStrict),
              ("codex".to_string(), McpIsolation::None),
          ]
          .into(),
      );
      hosts.record_mcp_isolation("host-1", &first).unwrap();
      assert_eq!(hosts.host("host-1").unwrap().unwrap().mcp_isolation, Some(first));
      let second = AgentIsolation([("codex".to_string(), McpIsolation::None)].into());
      hosts.record_mcp_isolation("host-1", &second).unwrap();
      assert_eq!(hosts.host("host-1").unwrap().unwrap().mcp_isolation, Some(second));
  }

  /// The host's own words, bounded: at most 32 agents, each id displayable
  /// and at most 64 bytes.
  #[test]
  fn a_hellos_mcp_isolation_is_bounded() {
      use hennery_kernel::hosts::MAX_AGENTS;
      use hennery_proto::frames::{AgentIsolation, McpIsolation};
      let hosts = Hosts::open_in_memory().unwrap();
      hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
      let mut many: std::collections::BTreeMap<String, McpIsolation> =
          (0..40).map(|i| (format!("agent-{i:02}"), McpIsolation::None)).collect();
      many.insert("a".repeat(65), McpIsolation::ClaudeStrict);
      many.insert("bidi\u{202e}".into(), McpIsolation::ClaudeStrict);
      many.insert(String::new(), McpIsolation::ClaudeStrict);
      hosts.record_mcp_isolation("host-1", &AgentIsolation(many)).unwrap();
      let kept = hosts.host("host-1").unwrap().unwrap().mcp_isolation.unwrap();
      assert_eq!(kept.0.len(), MAX_AGENTS);
      assert!(kept.0.keys().all(|agent| agent.starts_with("agent-")), "{kept:?}");
  }
  ```

In `crates/hennery-kernel/tests/owner.rs`, replace:

  ```rust
                  expected.insert("workspace_roots".into(), Value::Text("[]".into()));
              }
  ```

with:

  ```rust
                  expected.insert("workspace_roots".into(), Value::Text("[]".into()));
                  // Plan 8e's: no MCP isolation until a `hello` reports it.
                  expected.insert("mcp_isolation".into(), Value::Null);
              }
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          16,
  ```

with:

  ```rust
          17,
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test -p hennery-kernel --locked --test hosts
   ```

   It does not compile: `Hosts::record_mcp_isolation` and `HostRecord::mcp_isolation` do not exist.

   ```sh
   git add -A && git commit -m "test(kernel): a hello's per-agent MCP isolation is kept for the host"
   ```

- [ ] **Step 3: Implement**

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
  use hennery_proto::frames::Capabilities;
  ```

with:

  ```rust
  use hennery_proto::frames::{AgentIsolation, Capabilities};
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
  pub const MAX_ROOTS: usize = 32;

  ```

with:

  ```rust
  pub const MAX_ROOTS: usize = 32;

  /// The most agents of a `hello.mcp_isolation` the registry keeps (plan 8e).
  pub const MAX_AGENTS: usize = 32;

  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
      pub revoked_at: Option<i64>,
  }
  ```

with:

  ```rust
      pub revoked_at: Option<i64>,
      /// Per agent, how it isolates MCP servers, from its latest accepted
      /// `hello` (plan 8e decision E7); `None` until one is recorded.
      pub mcp_isolation: Option<AgentIsolation>,
  }
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust

      /// Store the workspace roots a reconciled connection of `host_id`
  ```

with:

  ```rust

      /// Store the per-agent MCP isolation of an accepted `hello` (plan 8e
      /// decision E7), for the host list while the host is away. The host's
      /// own words, bounded as the other reported fields are: at most
      /// `MAX_AGENTS` agents, each id displayable and at most 64 bytes;
      /// others are left out.
      pub fn record_mcp_isolation(&self, host_id: &str, isolation: &AgentIsolation) -> Result<()> {
          let kept = AgentIsolation(
              isolation
                  .0
                  .iter()
                  .filter(|(agent, _)| !agent.is_empty() && is_displayable_text(agent, 64))
                  .take(MAX_AGENTS)
                  .map(|(agent, how)| (agent.clone(), *how))
                  .collect(),
          );
          self.conn().execute(
              "UPDATE hosts SET mcp_isolation = ?2 WHERE id = ?1 AND owner_id = ?3",
              params![host_id, serde_json::to_string(&kept)?, self.owner],
          )?;
          Ok(())
      }

      /// Store the workspace roots a reconciled connection of `host_id`
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
  const HOST_COLUMNS: &str = "id, name, platform, host_version, capabilities, default_hat_id, created_at, last_seen_at, revoked_at, workspace_roots";
  ```

with:

  ```rust
  const HOST_COLUMNS: &str = "id, name, platform, host_version, capabilities, default_hat_id, created_at, last_seen_at, revoked_at, workspace_roots, mcp_isolation";
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
      let workspace_roots: String = r.get(9)?;
      Ok(HostRecord {
  ```

with:

  ```rust
      let workspace_roots: String = r.get(9)?;
      let mcp_isolation: Option<String> = r.get(10)?;
      Ok(HostRecord {
  ```

In `crates/hennery-kernel/src/hosts.rs`, replace:

  ```rust
          workspace_roots: serde_json::from_str(&workspace_roots).unwrap_or_default(),
      })
  ```

with:

  ```rust
          workspace_roots: serde_json::from_str(&workspace_roots).unwrap_or_default(),
          // Lenient, as a `hello` is read: an unknown mechanism is `none`.
          mcp_isolation: mcp_isolation.map(|json| serde_json::from_str(&json).unwrap_or_default()),
      })
  ```

In `crates/hennery-kernel/src/schema.rs`, replace:

  ```rust
      ",
  ];
  ```

with:

  ```rust
      ",
      // Plan 8e decision E7 (8c's decision 2 deferred it here): per agent,
      // how the host isolates its MCP servers, from its latest accepted
      // `hello`, so the host list shows it while the host is away. `NULL`:
      // no such `hello` recorded yet.
      "
      ALTER TABLE hosts ADD COLUMN mcp_isolation TEXT;
      ",
  ];
  ```

- [ ] **Step 4: Run the checks**

   Run the five checks:

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "feat(kernel): keep a hello's per-agent MCP isolation in the host registry"
   ```

### Task 3: The gateway: stdio sets, SessionMcp, a revoke ends open streams

**Files:** Create `crates/hennery-gateway/src/stdio.rs`, `src/revocation.rs`, `src/session.rs`, `tests/revocation.rs`, `tests/session_mcp.rs`, `tests/stdio.rs`; modify the gateway's `Cargo.toml`, `src/api.rs`, `crypto.rs`, `lib.rs`, `proxy.rs`, `schema.rs`, `store.rs`, `tokens.rs` and its tests; `crates/hennery-kernel/src/capabilities.rs`; `crates/hennery/src/main.rs`; `crates/hennery-testkit/tests/capabilities.rs`, `owner_filter.rs`.
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-gateway/tests/api.rs`, replace:

  ```rust
              operator: operator.clone(),
          });
  ```

with:

  ```rust
              operator: operator.clone(),
              revocations: Default::default(),
          });
  ```

In `crates/hennery-gateway/tests/api.rs`, replace:

  ```rust
          assert!(answer["message"].as_str().is_some_and(|m| !m.is_empty()), "{answer}");
      }
  }
  ```

with:

  ```rust
          assert!(answer["message"].as_str().is_some_and(|m| !m.is_empty()), "{answer}");
      }
  }

  /// Plan 8e (api-8e-8f A1): a stdio set is read without step-up and
  /// replaced with it; its values are never answered, only their names.
  #[tokio::test]
  async fn a_stdio_set_is_read_freely_and_replaced_behind_step_up() {
      let api = Api::new();
      api.host("host-a", 1);
      let hat = api.hat();
      let path = format!("/api/mcp/stdio-servers?host_id=host-a&hat_id={hat}");
      let fresh = api.session(0);
      let stale = api.session(600);
      let (status, set) = api.send(&stale, "GET", &path, None).await;
      assert_eq!(status, StatusCode::OK, "{set}");
      assert_eq!(set, json!({"host_id": "host-a", "hat_id": hat, "servers": []}));
      let body = json!({"servers": [{
          "name": "files", "command": "files-mcp", "args": ["--root", "/srv"],
          "env": [{"name": "FILES_KEY", "value": "s3cr3t-stdio-value"}, {"name": "EMPTY", "value": ""}]
      }]});
      let (status, refused) = api.send(&stale, "PUT", &path, Some(&body)).await;
      assert_eq!((status, code(&refused)), (StatusCode::FORBIDDEN, "step_up_required"));
      let (status, _) = api.send(&stale, "GET", &path, None).await;
      assert_eq!(status, StatusCode::OK);
      let (status, set) = api.send(&fresh, "PUT", &path, Some(&body)).await;
      assert_eq!(status, StatusCode::OK, "{set}");
      let server = &set["servers"][0];
      assert_eq!(server["name"], "files");
      assert_eq!(server["args"], json!(["--root", "/srv"]));
      assert_eq!(
          server["env"],
          json!([{"name": "FILES_KEY", "has_value": true}, {"name": "EMPTY", "has_value": true}])
      );
      assert!(server["created_at"].as_str().unwrap().ends_with('Z'), "{set}");
      let (_, read) = api.send(&stale, "GET", &path, None).await;
      assert_eq!(read, set);
      assert!(!read.to_string().contains("s3cr3t-stdio-value"), "{read}");
  }

  #[tokio::test]
  async fn the_stdio_routes_answer_their_codes() {
      let api = Api::new();
      api.host("host-a", 1);
      api.host("host-gone", 2);
      let hat = api.hat();
      let s = api.session(0);
      let at = |host: &str, hat: &str| format!("/api/mcp/stdio-servers?host_id={host}&hat_id={hat}");
      let long = "h".repeat(65);
      for path in [
          "/api/mcp/stdio-servers".to_string(),
          "/api/mcp/stdio-servers?host_id=host-a".to_string(),
          format!("/api/mcp/stdio-servers?hat_id={hat}"),
          at(&long, &hat),
          at("host-a", &long),
          at("", &hat),
      ] {
          let (status, body) = api.send(&s, "GET", &path, None).await;
          assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"), "{path}");
          assert!(!body.to_string().contains(&long), "{body}");
      }
      let (status, body) = api.send(&s, "GET", &at("host-x", &hat), None).await;
      assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "not_found"));
      let (status, body) = api.send(&s, "GET", &at("host-a", "hat-x"), None).await;
      assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "not_found"));
      let one = |name: &str, env: Value| json!({"servers": [{"name": name, "command": "c", "env": env}]});
      let (api, s) = (&api, &s);
      let put = |path: String, body: Value| async move { api.send(s, "PUT", &path, Some(&body)).await };
      let (status, body) = put(at("host-a", &hat), one("Bad", json!([]))).await;
      assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"));
      let (status, body) = put(at("host-a", &hat), one("files", json!([{"name": "K"}]))).await;
      assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "env_value_missing"));
      api.create("linear").await;
      let (status, body) = put(at("host-a", &hat), one("linear", json!([]))).await;
      assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "slug_taken"));
      let many: Vec<Value> = (0..33)
          .map(|i| json!({"name": format!("s{i}"), "command": "c"}))
          .collect();
      let (status, body) = put(at("host-a", &hat), json!({ "servers": many })).await;
      assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "too_many_stdio_servers"));
      let (status, body) = put(at("host-a", &hat), json!({"servers": [], "extra": 1})).await;
      assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
      api.hosts.revoke("host-gone", unix_now()).unwrap();
      let (status, body) = put(at("host-gone", &hat), json!({"servers": []})).await;
      assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "host_revoked"));
      let (status, body) = api.send(s, "GET", &at("host-gone", &hat), None).await;
      assert_eq!(status, StatusCode::OK, "{body}");
      // And the other way: a connection may not take a stdio server's name.
      let (status, _) = put(at("host-a", &hat), one("files", json!([]))).await;
      assert_eq!(status, StatusCode::OK);
      let (status, body) = api
          .send(s, "POST", "/api/mcp/connections", Some(&api.new_body("files")))
          .await;
      assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "slug_taken"));
  }
  ```

In `crates/hennery-gateway/tests/api_log.rs`, replace:

  ```rust
          key: key.clone(),
          operator,
      });
      let send = |method: &str, path: &str, body: String| {
  ```

with:

  ```rust
          key: key.clone(),
          operator,
          revocations: Default::default(),
      });
      let send = |method: &str, path: &str, body: String| {
  ```

In `crates/hennery-gateway/tests/api_log.rs`, replace:

  ```rust
          operator,
      });
  ```

with:

  ```rust
          operator,
          revocations: Default::default(),
      });
  ```

In `crates/hennery-gateway/tests/owner.rs`, replace:

  ```rust
      store.purge_hat(THEIR_HAT).unwrap();
  ```

with:

  ```rust
      let _ = store.purge_hat(THEIR_HAT).unwrap();
  ```

In `crates/hennery-gateway/tests/owner.rs`, replace:

  ```rust
      store.purge_hat(&hat).unwrap();
  ```

with:

  ```rust
      let _ = store.purge_hat(&hat).unwrap();
  ```

Create `crates/hennery-gateway/tests/revocation.rs`:

  ```rust
  //! A revoke ends what is open on its token (plan 8e decision 12, the fleet
  //! parent's ruling): an event stream already flowing is cut, a request
  //! still waiting answers 404, and another token's stream goes on. Every
  //! wait has a positive signal; the bounds are failure bounds, not sleeps.

  mod support;

  use axum::body::{Body, Bytes};
  use axum::http::{StatusCode, header};
  use axum::response::Response;
  use futures::StreamExt;
  use hennery_gateway::model::CredKind;
  use hennery_gateway::proxy::Limits;
  use hennery_gateway::session::{GatewayMcp, SessionMcp};
  use std::time::Duration;
  use support::upstream::{FakeUpstream, Harness};
  use tokio::sync::watch;

  /// How long anything here may take before the test fails.
  const BOUND: Duration = Duration::from_secs(10);

  /// An event stream that sends one event, then a second once `next` is
  /// signalled, then holds until the fake is dropped.
  fn two_events(next: watch::Receiver<bool>, hold: &watch::Receiver<()>) -> Response {
      let hold = hold.clone();
      let stream = futures::stream::unfold(0, move |step| {
          let mut next = next.clone();
          let mut hold = hold.clone();
          async move {
              match step {
                  0 => Some((Ok::<_, std::io::Error>(Bytes::from("data: {\"n\":1}\n\n")), 1)),
                  1 => {
                      while !*next.borrow_and_update() {
                          if next.changed().await.is_err() {
                              return None;
                          }
                      }
                      Some((Ok(Bytes::from("data: {\"n\":2}\n\n")), 2))
                  }
                  _ => {
                      while hold.changed().await.is_ok() {}
                      None
                  }
              }
          }
      });
      Response::builder()
          .header(header::CONTENT_TYPE, "text/event-stream")
          .body(Body::from_stream(stream))
          .unwrap()
  }

  struct Open {
      chunks: futures::stream::BoxStream<'static, reqwest::Result<Bytes>>,
  }

  impl Open {
      /// The next chunk, or `None` once the stream ended or broke.
      async fn next(&mut self) -> Option<Bytes> {
          match tokio::time::timeout(BOUND, self.chunks.next()).await {
              Ok(Some(Ok(bytes))) => Some(bytes),
              Ok(Some(Err(_)) | None) => None,
              Err(_) => panic!("neither a chunk nor an end within {BOUND:?}"),
          }
      }
  }

  async fn open(h: &Harness, token: &str) -> Open {
      let response = h
          .client
          .get(h.url("linear"))
          .bearer_auth(token)
          .header(header::ACCEPT, "text/event-stream")
          .send()
          .await
          .unwrap();
      assert_eq!(response.status(), StatusCode::OK);
      Open {
          chunks: response.bytes_stream().boxed(),
      }
  }

  /// A connection `linear` on `host-a` in the default hat, answered by
  /// `upstream`.
  fn mounted(h: &Harness, upstream: &FakeUpstream) {
      h.host("host-a", 1);
      let id = h.connection("linear", &upstream.url("/mcp"), CredKind::None);
      h.mount(&id, &["host-a"]);
  }

  fn revoke(h: &Harness, session: &str) {
      let mcp = GatewayMcp::new(&h.gateway());
      let mut conn = h.raw();
      let tx = conn.transaction().unwrap();
      let cut = mcp.revoke_in(&tx, session).unwrap();
      tx.commit().unwrap();
      mcp.cut(cut);
  }

  #[tokio::test]
  async fn a_revoke_cuts_the_tokens_open_stream_and_not_another_tokens() {
      let upstream = FakeUpstream::start().await;
      let (next, wait) = watch::channel(false);
      upstream.reply(move |_, hold| two_events(wait.clone(), hold));
      let h = Harness::new().await;
      mounted(&h, &upstream);
      let hat = h.hat();
      let mine = h.mint("s1", "host-a", &hat);
      let theirs = h.mint("s2", "host-a", &hat);
      let mut cut = open(&h, &mine).await;
      let mut kept = open(&h, &theirs).await;
      assert!(cut.next().await.is_some(), "the first event");
      assert!(kept.next().await.is_some(), "the first event");
      revoke(&h, "s1");
      // Cut: the stream ends (broken) without its second event.
      assert_eq!(cut.next().await, None);
      // The other token's stream still flows: its second event arrives.
      next.send(true).unwrap();
      let second = kept.next().await.expect("the other stream goes on");
      assert!(String::from_utf8_lossy(&second).contains("\"n\":2"), "{second:?}");
  }

  /// A request on the token still being read when the revoke commits ends at
  /// once with the same 404 as an unknown token, not at its body's timeout.
  #[tokio::test]
  async fn a_revoke_ends_a_request_not_yet_answered() {
      let upstream = FakeUpstream::start().await;
      let h = Harness::with_limits(Limits::new(8, 8, Duration::from_secs(60), Duration::from_secs(60))).await;
      mounted(&h, &upstream);
      let token = h.mint("s1", "host-a", &h.hat());
      // A body that never comes: the request waits in the proxy.
      let (_keep, never) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(1);
      let body = reqwest::Body::wrap_stream(tokio_stream_from(never));
      let request = h
          .client
          .post(h.url("linear"))
          .bearer_auth(&token)
          .header(header::CONTENT_TYPE, "application/json")
          .body(body)
          .send();
      let request = tokio::spawn(request);
      // Positive signal: the proxy is watching the token.
      let deadline = tokio::time::Instant::now() + BOUND;
      while h.revocations.watched() == 0 {
          assert!(
              tokio::time::Instant::now() < deadline,
              "the request never reached the proxy"
          );
          tokio::time::sleep(Duration::from_millis(10)).await;
      }
      revoke(&h, "s1");
      let response = tokio::time::timeout(BOUND, request).await.unwrap().unwrap().unwrap();
      assert_eq!(response.status(), StatusCode::NOT_FOUND);
      assert!(upstream.seen().is_empty(), "nothing went upstream");
  }

  /// The watch goes when the request does: nothing is kept per token once
  /// its answers have ended.
  #[tokio::test]
  async fn a_finished_request_leaves_no_watch() {
      let upstream = FakeUpstream::start().await;
      let h = Harness::new().await;
      mounted(&h, &upstream);
      let token = h.mint("s1", "host-a", &h.hat());
      let response = h.post("linear", &token, &support::upstream::list(1)).await;
      assert_eq!(response.status(), StatusCode::OK);
      response.bytes().await.unwrap();
      assert_eq!(h.revocations.watched(), 0);
      // An unknown or malformed token is never watched.
      let response = h.post("linear", "not-a-token", &support::upstream::list(1)).await;
      assert_eq!(response.status(), StatusCode::NOT_FOUND);
      assert_eq!(h.revocations.watched(), 0);
  }

  fn tokio_stream_from(
      mut rx: tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>,
  ) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
      futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
  }
  ```

Create `crates/hennery-gateway/tests/session_mcp.rs`:

  ```rust
  //! What a session gets of the gateway (ACP core §1, gateway spec §3.2;
  //! plan 8e): its hat's connections mounted on its host, `hennery-<slug>`
  //! at `<public_url>/mcp/<slug>` with a token minted in the caller's
  //! transaction, then its stdio servers; and what each revoke cuts.

  mod support;

  use hennery_gateway::model::CredKind;
  use hennery_gateway::revocation::Cut;
  use hennery_gateway::session::{GatewayMcp, SessionMcp, SessionRef};
  use hennery_gateway::stdio::StdioInput;
  use hennery_gateway::tokens::is_session_token;
  use hennery_kernel::operator::SetupOutcome;
  use hennery_kernel::secret::unix_now;
  use hennery_proto::frames::McpServer;
  use hennery_proto::rest::McpSessionDeliveryMode;
  use support::World;

  const ORIGIN: &str = "https://hennery.example";

  /// A world set up at `ORIGIN`, so the gateway has a `public_url`.
  fn world() -> World {
      let w = World::new();
      let operator = hennery_kernel::operator::Operator::open(&w.db).unwrap();
      let now = unix_now();
      let setup = operator.issue_setup_token(now).unwrap().unwrap();
      let SetupOutcome::Done { .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
          panic!("setup failed");
      };
      w
  }

  fn session<'a>(id: &'a str, host: &'a str, hat: &'a str) -> SessionRef<'a> {
      SessionRef {
          session_id: id,
          host_id: host,
          hat_id: hat,
      }
  }

  /// `servers_in` in a transaction of its own, committed, then cut.
  fn deliver(w: &World, mcp: &GatewayMcp, s: SessionRef<'_>, mode: McpSessionDeliveryMode) -> (Vec<McpServer>, usize) {
      let mut conn = w.raw();
      let tx = conn.transaction().unwrap();
      let delivered = mcp.servers_in(&tx, s, mode).unwrap();
      tx.commit().unwrap();
      let cut = delivered.cut.len();
      mcp.cut(delivered.cut);
      (delivered.servers, cut)
  }

  fn token_of(servers: &[McpServer]) -> String {
      let McpServer::Http { headers, .. } = &servers[0] else {
          panic!("{servers:?}");
      };
      headers[0].value.strip_prefix("Bearer ").unwrap().to_string()
  }

  fn http(name: &str, url: &str, token: &str) -> McpServer {
      McpServer::Http {
          name: name.into(),
          url: url.into(),
          headers: vec![hennery_proto::frames::NameValue::new(
              "Authorization",
              format!("Bearer {token}"),
          )],
      }
  }

  #[test]
  fn a_session_gets_its_hats_mounted_connections_then_its_stdio_servers() {
      let w = world();
      w.host("host-a", 1);
      w.host("host-b", 2);
      let hat = w.hat();
      let work = w.other_hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      let notes = w.connection_in("notes", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      let elsewhere = w.connection_in("elsewhere", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      let theirs = w.connection_in("theirs", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
      w.mount(&linear, &["host-a"]);
      w.mount(&notes, &["host-a", "host-b"]);
      w.mount(&elsewhere, &["host-b"]);
      w.mount(&theirs, &["host-a"]);
      let files = StdioInput {
          name: "files".into(),
          command: "files-mcp".into(),
          args: vec!["--root".into(), "/srv".into()],
          env: vec![("KEY".into(), Some("v".into()))],
      };
      let _ = w.store.replace_stdio_set("host-a", &hat, &[files], &w.key, 1).unwrap();
      let mcp = GatewayMcp::new(&w.gateway());
      let (servers, cut) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      assert_eq!(cut, 0, "a first start supersedes nothing");
      let token = token_of(&servers);
      assert!(is_session_token(&token));
      assert_eq!(
          servers,
          vec![
              http("hennery-linear", &format!("{ORIGIN}/mcp/linear"), &token),
              http("hennery-notes", &format!("{ORIGIN}/mcp/notes"), &token),
              McpServer::Stdio {
                  name: "hennery-files".into(),
                  command: "files-mcp".into(),
                  args: vec!["--root".into(), "/srv".into()],
                  env: vec![hennery_proto::frames::NameValue::new("KEY", "v")],
              },
          ]
      );
      let principal = w.proxy_store_resolve(&token).unwrap();
      assert_eq!(principal.hat_id, hat);
  }

  #[test]
  fn unisolated_delivers_as_isolated_does() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (servers, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Unisolated,
      );
      assert_eq!(servers.len(), 1);
      assert!(w.proxy_store_resolve(&token_of(&servers)).is_some());
  }

  /// Fallback and unsupported get nothing, and mint nothing: no token row.
  #[test]
  fn a_mode_that_does_not_deliver_mints_nothing() {
      for mode in [McpSessionDeliveryMode::Fallback, McpSessionDeliveryMode::Unsupported] {
          let w = world();
          w.host("host-a", 1);
          let hat = w.hat();
          let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
          w.mount(&linear, &["host-a"]);
          let _ = w.store.replace_stdio_set(
              "host-a",
              &hat,
              &[StdioInput {
                  name: "files".into(),
                  command: "c".into(),
                  args: vec![],
                  env: vec![],
              }],
              &w.key,
              1,
          );
          let mcp = GatewayMcp::new(&w.gateway());
          let (servers, _) = deliver(&w, &mcp, session("s1", "host-a", &hat), mode);
          assert!(servers.is_empty(), "{mode:?}: {servers:?}");
          assert_eq!(w.token_rows(), 0, "{mode:?}");
      }
  }

  /// No connection mounted for the hat: no token is minted at all, only the
  /// stdio servers go.
  #[test]
  fn no_connection_no_token() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let mcp = GatewayMcp::new(&w.gateway());
      let (servers, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      assert!(servers.is_empty());
      assert_eq!(w.token_rows(), 0);
  }

  /// A revoked host's mounts are not delivered (plan 8a decision 12).
  #[test]
  fn a_revoked_hosts_mounts_are_not_delivered() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      w.hosts.revoke("host-a", unix_now()).unwrap();
      let mcp = GatewayMcp::new(&w.gateway());
      let (servers, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      assert!(servers.is_empty(), "{servers:?}");
  }

  /// A session from before hats has no hat to deliver.
  #[test]
  fn a_session_without_a_hat_gets_nothing() {
      let w = world();
      w.host("host-a", 1);
      let mcp = GatewayMcp::new(&w.gateway());
      let (servers, _) = deliver(&w, &mcp, session("s1", "host-a", ""), McpSessionDeliveryMode::Isolated);
      assert!(servers.is_empty());
  }

  /// A resume mints a fresh token: the old one no longer resolves, and is
  /// what the resume's transaction names to cut.
  #[test]
  fn a_resume_supersedes_and_cuts_the_old_token() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (first, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let old = token_of(&first);
      let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(old.as_bytes()));
      let (second, cut) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let new = token_of(&second);
      assert_ne!(old, new);
      assert_eq!(cut, 1);
      assert!(watch.token().is_cancelled(), "the old token's watch is cut");
      assert!(w.proxy_store_resolve(&old).is_none());
      assert!(w.proxy_store_resolve(&new).is_some());
      // A resume that delivers nothing revokes the token, and cuts it.
      let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(new.as_bytes()));
      let (none, cut) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Fallback,
      );
      assert!(none.is_empty());
      assert_eq!(cut, 1);
      assert!(watch.token().is_cancelled());
      assert!(w.proxy_store_resolve(&new).is_none());
  }

  /// Nothing is cut before the caller commits: the cut is the caller's, and
  /// a transaction rolled back leaves the token working, its streams too.
  #[test]
  fn a_rolled_back_mint_leaves_no_token_and_cuts_nothing() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (first, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let old = token_of(&first);
      let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(old.as_bytes()));
      {
          let mut conn = w.raw();
          let tx = conn.transaction().unwrap();
          let delivered = mcp
              .servers_in(&tx, session("s1", "host-a", &hat), McpSessionDeliveryMode::Isolated)
              .unwrap();
          // Rolled back: dropped uncommitted, and its cut is never made.
          drop(tx);
          drop(delivered);
      }
      assert!(!watch.token().is_cancelled());
      assert!(w.proxy_store_resolve(&old).is_some());
  }

  #[test]
  fn revoke_in_cuts_a_live_token_and_only_that() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (one, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let (two, _) = deliver(
          &w,
          &mcp,
          session("s2", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let revoke = |id: &str| -> Cut {
          let mut conn = w.raw();
          let tx = conn.transaction().unwrap();
          let cut = mcp.revoke_in(&tx, id).unwrap();
          tx.commit().unwrap();
          cut
      };
      let cut = revoke("s1");
      assert_eq!(cut.len(), 1);
      mcp.cut(cut);
      assert!(w.proxy_store_resolve(&token_of(&one)).is_none());
      assert!(w.proxy_store_resolve(&token_of(&two)).is_some());
      assert!(revoke("s1").is_empty(), "twice: nothing live to cut");
      assert!(revoke("never").is_empty());
  }

  #[test]
  fn revoke_host_in_cuts_every_live_token_of_the_host() {
      let w = world();
      w.host("host-a", 1);
      w.host("host-b", 2);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a", "host-b"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (a1, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let (a2, _) = deliver(
          &w,
          &mcp,
          session("s2", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let (b, _) = deliver(
          &w,
          &mcp,
          session("s3", "host-b", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let mut conn = w.raw();
      let tx = conn.transaction().unwrap();
      let cut = mcp.revoke_host_in(&tx, "host-a").unwrap();
      tx.commit().unwrap();
      assert_eq!(cut.len(), 2);
      mcp.cut(cut);
      for token in [&a1, &a2] {
          assert!(w.token_revoked(&token_of(token)));
      }
      assert!(!w.token_revoked(&token_of(&b)));
  }

  #[test]
  fn a_hat_purge_cuts_its_tokens() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let work = w.other_hat();
      let theirs = w.connection_in("theirs", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
      w.mount(&theirs, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (servers, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &work),
          McpSessionDeliveryMode::Isolated,
      );
      let token = token_of(&servers);
      let watch = w
          .revocations
          .watch(&hennery_kernel::secret::sha256_hex(token.as_bytes()));
      mcp.purge_hat(&work).unwrap();
      assert!(watch.token().is_cancelled());
      assert!(w.proxy_store_resolve(&token).is_none());
      // The default hat is not touched.
      let _ = hat;
  }

  /// No `public_url`: a session with a connection to reach cannot be given
  /// one, and its transition should roll back rather than start without.
  #[test]
  fn without_a_public_url_a_mint_fails() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let mut conn = w.raw();
      let tx = conn.transaction().unwrap();
      let err = mcp
          .servers_in(&tx, session("s1", "host-a", &hat), McpSessionDeliveryMode::Isolated)
          .unwrap_err();
      assert!(format!("{err:#}").contains("public_url"), "{err:#}");
  }

  /// A resume after the hat's last connection was unmounted mints none, and
  /// the token before it is revoked and cut, not left live.
  #[test]
  fn a_resume_with_nothing_mounted_revokes_the_old_token() {
      let w = world();
      w.host("host-a", 1);
      let hat = w.hat();
      let linear = w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &hat, None);
      w.mount(&linear, &["host-a"]);
      let mcp = GatewayMcp::new(&w.gateway());
      let (first, _) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      let old = token_of(&first);
      let watch = w.revocations.watch(&hennery_kernel::secret::sha256_hex(old.as_bytes()));
      w.mount(&linear, &[]);
      let (none, cut) = deliver(
          &w,
          &mcp,
          session("s1", "host-a", &hat),
          McpSessionDeliveryMode::Isolated,
      );
      assert!(none.is_empty());
      assert_eq!(cut, 1);
      assert!(watch.token().is_cancelled());
      assert!(w.token_revoked(&old));
  }
  ```

Create `crates/hennery-gateway/tests/stdio.rs`:

  ```rust
  //! Local stdio servers (gateway spec §3.4; plan 8e decisions E1–E5): one
  //! set per (host, hat), replaced whole; values write-only, sealed per row
  //! and bound to its host and hat; names disjoint from connection slugs both
  //! ways; a hat's purge takes them.

  mod support;

  use hennery_gateway::model::{Change, CredKind};
  use hennery_gateway::stdio::{MAX_OWNER, StdioChange, StdioInput, StdioServer};
  use hennery_kernel::secret::unix_now;
  use support::World;

  const SECRET: &str = "files-s3cr3t-0123";

  fn input(name: &str, env: &[(&str, Option<&str>)]) -> StdioInput {
      StdioInput {
          name: name.into(),
          command: "/usr/local/bin/files-mcp".into(),
          args: vec!["--root".into(), "/srv".into()],
          env: env
              .iter()
              .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
              .collect(),
      }
  }

  fn done(change: StdioChange) -> Vec<StdioServer> {
      match change {
          StdioChange::Done(set) => set,
          other => panic!("not done: {other:?}"),
      }
  }

  /// The values a session would get, through the delivery read.
  fn delivered(w: &World, host: &str, hat: &str) -> Vec<hennery_proto::frames::McpServer> {
      let mcp = hennery_gateway::session::GatewayMcp::new(&w.gateway());
      let mut conn = w.raw();
      let tx = conn.transaction().unwrap();
      use hennery_gateway::session::{SessionMcp, SessionRef};
      let delivered = mcp
          .servers_in(
              &tx,
              SessionRef {
                  session_id: "s-probe",
                  host_id: host,
                  hat_id: hat,
              },
              hennery_proto::rest::McpSessionDeliveryMode::Isolated,
          )
          .unwrap();
      mcp.cut(delivered.cut);
      delivered.servers
  }

  fn env_of(servers: &[hennery_proto::frames::McpServer], name: &str) -> Vec<(String, String)> {
      servers
          .iter()
          .find_map(|s| match s {
              hennery_proto::frames::McpServer::Stdio { name: n, env, .. } if n == name => {
                  Some(env.iter().map(|pair| (pair.name.clone(), pair.value.clone())).collect())
              }
              _ => None,
          })
          .unwrap_or_else(|| panic!("no {name}"))
  }

  #[test]
  fn a_set_is_stored_listed_and_replaced_whole() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      let set = done(
          w.store
              .replace_stdio_set(
                  "host-a",
                  &hat,
                  &[input("files", &[("KEY", Some(SECRET))]), input("notes", &[])],
                  &w.key,
                  100,
              )
              .unwrap(),
      );
      assert_eq!(
          set.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
          ["files", "notes"]
      );
      assert_eq!(set[0].env, ["KEY"]);
      assert_eq!(set[0].args, ["--root", "/srv"]);
      assert_eq!((set[0].created_at, set[0].updated_at), (100, 100));
      assert_eq!(done(w.store.stdio_set("host-a", &hat).unwrap()), set);
      // Another (host, hat) has its own set: none.
      assert!(done(w.store.stdio_set("host-a", &w.other_hat()).unwrap()).is_empty());
      // Replaced whole: `notes` goes, `files` keeps its row and its created_at.
      let set = done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[("KEY", None)])], &w.key, 200)
              .unwrap(),
      );
      assert_eq!(set.len(), 1);
      assert_eq!((set[0].created_at, set[0].updated_at), (100, 100), "nothing changed");
      assert_eq!(
          env_of(&delivered(&w, "host-a", &hat), "hennery-files"),
          [("KEY".to_string(), SECRET.to_string())]
      );
      // `[]` deletes them all.
      assert!(done(w.store.replace_stdio_set("host-a", &hat, &[], &w.key, 300).unwrap()).is_empty());
  }

  /// Decision E3: absent keeps the stored value, a string sets it (`""` too),
  /// a name left out is deleted.
  #[test]
  fn values_are_kept_set_and_deleted_by_name() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      let put = |env: &[(&str, Option<&str>)]| {
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", env)], &w.key, unix_now())
              .unwrap()
      };
      done(put(&[("A", Some("one")), ("B", Some("two"))]));
      done(put(&[("A", None), ("B", Some("")), ("C", Some("three"))]));
      assert_eq!(
          env_of(&delivered(&w, "host-a", &hat), "hennery-files"),
          [
              ("A".to_string(), "one".to_string()),
              ("B".to_string(), String::new()),
              ("C".to_string(), "three".to_string())
          ]
      );
      done(put(&[("C", None)]));
      assert_eq!(
          env_of(&delivered(&w, "host-a", &hat), "hennery-files"),
          [("C".to_string(), "three".to_string())]
      );
      // A deleted name cannot be kept.
      assert!(matches!(put(&[("A", None)]), StdioChange::EnvValueMissing(_)));
  }

  #[test]
  fn a_value_never_stored_cannot_be_kept_and_nothing_changes() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some("one"))])], &w.key, 1)
              .unwrap(),
      );
      let refused = w
          .store
          .replace_stdio_set(
              "host-a",
              &hat,
              &[input("files", &[("A", None), ("NEW", None)]), input("other", &[])],
              &w.key,
              2,
          )
          .unwrap();
      let StdioChange::EnvValueMissing(why) = refused else {
          panic!("{refused:?}");
      };
      assert!(why.contains("files") && why.contains("NEW"), "{why}");
      let set = done(w.store.stdio_set("host-a", &hat).unwrap());
      assert_eq!(set.len(), 1, "a refused set changes nothing");
  }

  /// S1: a kept value needs its server's command unchanged.
  #[test]
  fn a_changed_command_must_resend_its_values() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some(SECRET))])], &w.key, 1)
              .unwrap(),
      );
      let mut moved = input("files", &[("A", None)]);
      moved.command = "/tmp/evil".into();
      assert!(matches!(
          w.store
              .replace_stdio_set("host-a", &hat, &[moved.clone()], &w.key, 2)
              .unwrap(),
          StdioChange::EnvValueMissing(_)
      ));
      moved.env = vec![("A".into(), Some("new".into()))];
      done(w.store.replace_stdio_set("host-a", &hat, &[moved], &w.key, 3).unwrap());
  }

  /// R5: a kept value is read only from the same set: the same name in
  /// another hat or on another host keeps nothing.
  #[test]
  fn a_kept_value_never_comes_from_another_hat_or_host() {
      let w = World::new();
      w.host("host-a", 1);
      w.host("host-b", 2);
      let hat = w.hat();
      let work = w.other_hat();
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some(SECRET))])], &w.key, 1)
              .unwrap(),
      );
      for (host, hat) in [("host-a", work.as_str()), ("host-b", hat.as_str())] {
          assert!(
              matches!(
                  w.store
                      .replace_stdio_set(host, hat, &[input("files", &[("A", None)])], &w.key, 2)
                      .unwrap(),
                  StdioChange::EnvValueMissing(_)
              ),
              "{host} {hat}"
          );
      }
  }

  /// R5: a blob copied to another host's or hat's row does not open there.
  #[test]
  fn a_blob_moved_to_another_row_does_not_open() {
      let w = World::new();
      w.host("host-a", 1);
      w.host("host-b", 2);
      let hat = w.hat();
      for host in ["host-a", "host-b"] {
          let value = if host == "host-a" { SECRET } else { "other" };
          done(
              w.store
                  .replace_stdio_set(host, &hat, &[input("files", &[("A", Some(value))])], &w.key, 1)
                  .unwrap(),
          );
      }
      w.raw()
          .execute(
              "UPDATE gw_stdio_servers SET env_ciphertext =
                   (SELECT env_ciphertext FROM gw_stdio_servers WHERE host_id = 'host-a')
               WHERE host_id = 'host-b'",
              [],
          )
          .unwrap();
      let mcp = hennery_gateway::session::GatewayMcp::new(&w.gateway());
      let mut conn = w.raw();
      let tx = conn.transaction().unwrap();
      use hennery_gateway::session::{SessionMcp, SessionRef};
      let err = mcp
          .servers_in(
              &tx,
              SessionRef {
                  session_id: "s1",
                  host_id: "host-b",
                  hat_id: &hat,
              },
              hennery_proto::rest::McpSessionDeliveryMode::Isolated,
          )
          .unwrap_err();
      assert!(!format!("{err:#}").contains(SECRET), "{err:#}");
  }

  /// Decision E2: a stdio name and a connection slug never collide, in
  /// either direction, whatever their hats.
  #[test]
  fn names_and_slugs_are_one_namespace_both_ways() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      let work = w.other_hat();
      w.connection_in("linear", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
      let refused = w
          .store
          .replace_stdio_set("host-a", &hat, &[input("linear", &[])], &w.key, 1)
          .unwrap();
      assert!(matches!(refused, StdioChange::SlugTaken(_)), "{refused:?}");
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[])], &w.key, 1)
              .unwrap(),
      );
      let mut new = support::new_connection("files", &work);
      new.internal_network = true;
      assert!(matches!(w.store.create(&new, 1).unwrap(), Change::SlugTaken));
  }

  /// Q1's default: the same name may repeat across (host, hat) sets.
  #[test]
  fn a_name_may_repeat_in_another_set() {
      let w = World::new();
      w.host("host-a", 1);
      w.host("host-b", 2);
      let hat = w.hat();
      for host in ["host-a", "host-b"] {
          done(
              w.store
                  .replace_stdio_set(host, &hat, &[input("files", &[])], &w.key, 1)
                  .unwrap(),
          );
      }
  }

  #[test]
  fn an_unknown_host_or_hat_is_not_found_and_a_revoked_host_is_read_only() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      assert_eq!(w.store.stdio_set("host-x", &hat).unwrap(), StdioChange::NotFound);
      assert_eq!(w.store.stdio_set("host-a", "hat-x").unwrap(), StdioChange::NotFound);
      assert_eq!(
          w.store.replace_stdio_set("host-x", &hat, &[], &w.key, 1).unwrap(),
          StdioChange::NotFound
      );
      assert_eq!(
          w.store.replace_stdio_set("host-a", "hat-x", &[], &w.key, 1).unwrap(),
          StdioChange::NotFound
      );
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[])], &w.key, 1)
              .unwrap(),
      );
      w.hosts.revoke("host-a", 2).unwrap();
      assert_eq!(done(w.store.stdio_set("host-a", &hat).unwrap()).len(), 1);
      assert_eq!(
          w.store.replace_stdio_set("host-a", &hat, &[], &w.key, 3).unwrap(),
          StdioChange::HostRevoked
      );
  }

  /// A hat frozen for its purge takes no new set: its sets go with the purge
  /// (kernel spec §5.5), and one written after would keep the hat's row.
  #[test]
  fn a_hat_being_purged_takes_no_set() {
      let w = World::new();
      w.host("host-a", 1);
      let work = w.other_hat();
      w.raw()
          .execute(
              "INSERT INTO purged_hats(hat_id, owner_id, purged_at) VALUES (?1, ?2, 1)",
              [&work, w.store.owner_id()],
          )
          .unwrap();
      assert_eq!(
          w.store
              .replace_stdio_set("host-a", &work, &[input("files", &[])], &w.key, 1)
              .unwrap(),
          StdioChange::NotFound
      );
  }

  #[test]
  fn the_owner_has_at_most_1024() {
      let w = World::new();
      let hat = w.hat();
      let sets = MAX_OWNER / 32;
      for i in 0..sets {
          let host = format!("host-{i}");
          w.host(&host, (i + 1) as u8);
          let servers: Vec<StdioInput> = (0..32).map(|j| input(&format!("s{j}"), &[])).collect();
          done(w.store.replace_stdio_set(&host, &hat, &servers, &w.key, 1).unwrap());
      }
      w.host("host-last", 200);
      assert!(matches!(
          w.store
              .replace_stdio_set("host-last", &hat, &[input("one", &[])], &w.key, 1)
              .unwrap(),
          StdioChange::TooMany(_)
      ));
      // Replacing a full set with as many is not more.
      let servers: Vec<StdioInput> = (0..32).map(|j| input(&format!("t{j}"), &[])).collect();
      done(w.store.replace_stdio_set("host-0", &hat, &servers, &w.key, 2).unwrap());
  }

  /// Gateway spec §2, kernel §5.5: a hat's purge takes its sets, so the hat
  /// can go; another hat's stay.
  #[test]
  fn a_hat_purge_takes_its_sets() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      let work = w.other_hat();
      for hat in [&hat, &work] {
          done(
              w.store
                  .replace_stdio_set("host-a", hat, &[input("files", &[("A", Some("v"))])], &w.key, 1)
                  .unwrap(),
          );
      }
      let _ = w.store.purge_hat(&work).unwrap();
      assert_eq!(
          w.raw()
              .execute(
                  "DELETE FROM hats WHERE id = ?1 AND owner_id = ?2",
                  [&work, w.store.owner_id()]
              )
              .unwrap(),
          1
      );
      assert_eq!(done(w.store.stdio_set("host-a", &hat).unwrap()).len(), 1);
  }

  /// Plan 8a's hand-off: a stored stdio value counts as ciphertext, so a
  /// missing master key is an error, and the newest one must open with the
  /// key the gateway starts with.
  #[test]
  fn stdio_values_count_as_ciphertext_and_are_checked_against_the_key() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      assert!(!w.store.has_ciphertext().unwrap());
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[])], &w.key, 1)
              .unwrap(),
      );
      assert!(!w.store.has_ciphertext().unwrap(), "no values, nothing sealed");
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some("v"))])], &w.key, 2)
              .unwrap(),
      );
      assert!(w.store.has_ciphertext().unwrap());
      w.store.check_key(&w.key).unwrap();
      let other = hennery_gateway::key::MasterKey::from_bytes([9; 32]);
      let err = w.store.check_key(&other).unwrap_err();
      assert!(format!("{err:#}").contains("stdio server"), "{err:#}");
  }

  #[test]
  fn values_are_sealed_at_rest() {
      let w = World::new();
      w.host("host-a", 1);
      let hat = w.hat();
      done(
          w.store
              .replace_stdio_set("host-a", &hat, &[input("files", &[("A", Some(SECRET))])], &w.key, 1)
              .unwrap(),
      );
      let raw = std::fs::read(&w.db).unwrap();
      let wal = std::fs::read(w.db.with_extension("db-wal")).unwrap_or_default();
      for bytes in [raw, wal] {
          assert!(
              !bytes.windows(SECRET.len()).any(|window| window == SECRET.as_bytes()),
              "a value is in the database in clear"
          );
      }
  }
  ```

In `crates/hennery-gateway/tests/store.rs`, replace:

  ```rust
      assert!(w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).is_err());
      w.store.purge_hat(&work).unwrap();
      assert_eq!(w.store.connection(&gone).unwrap(), None);
  ```

with:

  ```rust
      assert!(w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).is_err());
      let _ = w.store.purge_hat(&work).unwrap();
      assert_eq!(w.store.connection(&gone).unwrap(), None);
  ```

In `crates/hennery-gateway/tests/store.rs`, replace:

  ```rust
      w.store.purge_hat(&work).unwrap();
      w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).unwrap();
      w.store.purge_hat(&work).unwrap();
      w.store.purge_hat("hat-0000000000000000").unwrap();
  ```

with:

  ```rust
      let _ = w.store.purge_hat(&work).unwrap();
      w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).unwrap();
      let _ = w.store.purge_hat(&work).unwrap();
      let _ = w.store.purge_hat("hat-0000000000000000").unwrap();
  ```

In `crates/hennery-gateway/tests/store.rs`, replace:

  ```rust
      // Plan 8a's tables, then plan 8d's session tokens.
      assert_eq!(version, 2);
  ```

with:

  ```rust
      // Plan 8a's tables, then plan 8d's session tokens, then plan 8e's
      // stdio servers.
      assert_eq!(version, 3);
  ```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

  ```rust
  use hennery_gateway::key::MasterKey;
  use hennery_gateway::model::{Change, CredKind, CredentialChange, NewConnection};
  ```

with:

  ```rust
  use hennery_gateway::api::GatewayState;
  use hennery_gateway::key::MasterKey;
  use hennery_gateway::model::{Change, CredKind, CredentialChange, NewConnection};
  use hennery_gateway::revocation::Revocations;
  ```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

  ```rust
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::secret::unix_now;
  ```

with:

  ```rust
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_kernel::secret::unix_now;
  ```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

  ```rust
      pub hosts: Hosts,
  }
  ```

with:

  ```rust
      pub hosts: Hosts,
      /// Shared by the proxy (`Harness`) and `gateway()` (plan 8e).
      pub revocations: Revocations,
  }
  ```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

  ```rust
              hosts,
          }
  ```

with:

  ```rust
              hosts,
              revocations: Revocations::new(),
          }
  ```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

  ```rust
          })
      }
  ```

with:

  ```rust
          })
      }

      /// A connection in `hat` to a loopback upstream, not yet created.
      pub fn new_connection(&self, slug: &str, hat: &str) -> NewConnection {
          new_connection(slug, hat)
      }
  ```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

  ```rust
      }
  }
  ```

with:

  ```rust
      }

      /// The gateway on this database, as the collector builds it: its
      /// store, key and `revocations`, the owner opened on the same file.
      pub fn gateway(&self) -> GatewayState {
          GatewayState {
              store: self.store.clone(),
              key: self.key.clone(),
              operator: Arc::new(Operator::open(&self.db).unwrap()),
              revocations: self.revocations.clone(),
          }
      }

      /// What the proxy would resolve `token` to now.
      pub fn proxy_store_resolve(&self, token: &str) -> Option<hennery_gateway::scope::Principal> {
          use hennery_gateway::scope::ClientIdentity;
          self.proxy_store.resolve(token, unix_now()).unwrap()
      }

      /// How many session token rows there are, live or not.
      pub fn token_rows(&self) -> i64 {
          self.raw()
              .query_row("SELECT count(*) FROM gw_session_tokens", [], |r| r.get(0))
              .unwrap()
      }

      /// Whether `token`'s row is revoked (not whether it resolves: a revoked
      /// host's tokens stop resolving by the host's join alone).
      pub fn token_revoked(&self, token: &str) -> bool {
          self.raw()
              .query_row(
                  "SELECT revoked_at IS NOT NULL FROM gw_session_tokens WHERE token_hash = ?1",
                  [hennery_kernel::secret::sha256_hex(token.as_bytes())],
                  |r| r.get(0),
              )
              .unwrap()
      }
  }

  /// A `none` connection in `hat` to a loopback upstream, not yet created.
  pub fn new_connection(slug: &str, hat: &str) -> NewConnection {
      NewConnection {
          slug: slug.into(),
          label: format!("Label {slug}"),
          url: "http://127.0.0.1:9/mcp".into(),
          hat_id: hat.into(),
          cred_kind: CredKind::None,
          static_header: None,
          static_prefix: None,
          tool_allowlist: None,
          internal_network: true,
      }
  }
  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
          let app = router(ProxyState::full(
              world.proxy_store.clone(),
              world.store.clone(),
              world.key.clone(),
              egress,
              limits,
          ));
  ```

with:

  ```rust
          let gateway = world.gateway();
          let app = router(ProxyState::full(world.proxy_store.clone(), &gateway, egress, limits));
  ```

In `crates/hennery-gateway/tests/tokens.rs`, replace:

  ```rust
  /// run; without it, the tokens' foreign key keeps the hat.
  ```

with:

  ```rust
  /// run; without it, the tokens' foreign key keeps the hat. Since plan 8e,
  /// `GatewayStore::purge_hat` takes them itself (plan 8d's hand-off), and
  /// names both for the cut.
  ```

In `crates/hennery-gateway/tests/tokens.rs`, replace:

  ```rust
      h.store.purge_hat(&work).unwrap();
      assert!(delete_hat(&h).is_err(), "the hat went with tokens left");
      let mut conn = h.raw();
      let tx = conn.transaction().unwrap();
      assert_eq!(tokens::purge_hat_in(&tx, h.store.owner_id(), &work).unwrap(), 2);
  ```

with:

  ```rust
      {
          // Without the gateway's purge, the tokens keep the hat.
          let mut conn = h.raw();
          let tx = conn.transaction().unwrap();
          assert!(
              tx.execute(
                  "DELETE FROM hats WHERE id = ?1 AND owner_id = ?2",
                  [&work, h.store.owner_id()]
              )
              .is_err()
          );
      }
      assert_eq!(h.store.purge_hat(&work).unwrap().len(), 2);
      let mut conn = h.raw();
      let tx = conn.transaction().unwrap();
  ```

In `crates/hennery-gateway/tests/tokens.rs`, replace:

  ```rust
      assert_eq!(delete_hat(&h).unwrap(), 1);
  }
  ```

with:

  ```rust
      assert_eq!(delete_hat(&h).unwrap(), 1);
      // Idempotent: nothing left, nothing to cut.
      assert!(h.store.purge_hat(&work).unwrap().is_empty());
  }
  ```

In `crates/hennery-testkit/tests/capabilities.rs`, replace:

  ```rust
          serde_json::json!({"mode": "full", "features": ["mcp_connections"]})
  ```

with:

  ```rust
          serde_json::json!({"mode": "full", "features": ["mcp_connections", "mcp_stdio"]})
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          4,
  ```

with:

  ```rust
          7,
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          5,
      ),
  ];

  ```

with:

  ```rust
          5,
      ),
      // Plan 8e: what a session gets, and the stdio servers.
      (
          "hennery-gateway/src/session.rs",
          include_str!("../../hennery-gateway/src/session.rs"),
          1,
      ),
      (
          "hennery-gateway/src/stdio.rs",
          include_str!("../../hennery-gateway/src/stdio.rs"),
          7,
      ),
  ];

  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
  const GATEWAY_STATEMENTS: usize = 23;
  ```

with:

  ```rust
  const GATEWAY_STATEMENTS: usize = 25;
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test -p hennery-gateway --locked
   ```

   It does not compile: `hennery_gateway::{stdio, revocation, session}` do not exist, and `ProxyState::full` takes the gateway's state.

   ```sh
   git add -A && git commit -m "test(gateway): stdio sets, session tokens through the sessions' side, a revoke ends open streams"
   ```

- [ ] **Step 3: Implement**

In `crates/hennery-gateway/Cargo.toml`, replace:

  ```toml
  tokio.workspace = true
  tracing.workspace = true
  ```

with:

  ```toml
  tokio.workspace = true
  # A revoke ends the token's open streams (plan 8e decision 12).
  tokio-util.workspace = true
  tracing.workspace = true
  ```

In `crates/hennery-gateway/src/api.rs`, replace:

  ```rust
  use crate::store::GatewayStore;
  use axum::extract::{DefaultBodyLimit, Extension, Path, State};
  ```

with:

  ```rust
  use crate::stdio::{self, StdioChange, StdioInput};
  use crate::store::GatewayStore;
  use axum::extract::rejection::QueryRejection;
  use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
  ```

In `crates/hennery-gateway/src/api.rs`, replace:

  ```rust
      McpMountsRequest, UpdateMcpConnectionRequest,
  ```

with:

  ```rust
      McpMountsRequest, McpStdioEnvItem, McpStdioServerItem, McpStdioServerSet, McpStdioServersRequest,
      UpdateMcpConnectionRequest,
  ```

In `crates/hennery-gateway/src/api.rs`, replace:

  ```rust
      pub operator: Arc<Operator>,
  }
  ```

with:

  ```rust
      pub operator: Arc<Operator>,
      /// What a revoke cuts (plan 8e decision 12): one per collector, shared
      /// by the proxy (`ProxyState::full`) and the sessions' side
      /// (`session::GatewayMcp`).
      pub revocations: crate::revocation::Revocations,
  }
  ```

In `crates/hennery-gateway/src/api.rs`, replace:

  ```rust
              put(credential.layer(middleware::from_fn(require_step_up))),
          )
  ```

with:

  ```rust
              put(credential.layer(middleware::from_fn(require_step_up))),
          )
          // Plan 8e: a stdio server is a command the host runs, so a `PUT`
          // needs step-up (kernel spec §3.4); a `GET` does not.
          .route(
              "/api/mcp/stdio-servers",
              get(stdio_set).put(replace_stdio_set.layer(middleware::from_fn(require_step_up))),
          )
  ```

In `crates/hennery-gateway/src/api.rs`, replace:

  ```rust
          Ok(CredentialChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
          Err(err) => internal(err),
      }
  }
  ```

with:

  ```rust
          Ok(CredentialChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
          Err(err) => internal(err),
      }
  }

  /// `?host_id=&hat_id=` of the stdio routes. Read as text: a missing or
  /// malformed one is an `ApiError`.
  #[derive(serde::Deserialize)]
  struct StdioQuery {
      host_id: Option<String>,
      hat_id: Option<String>,
  }

  /// Both ids, each 1 to 64 bytes; neither is quoted back when refused.
  fn stdio_place(query: Result<Query<StdioQuery>, QueryRejection>) -> Result<(String, String), Box<Response>> {
      let refused = || {
          error(
              StatusCode::BAD_REQUEST,
              "invalid",
              format!("host_id and hat_id are each 1 to {} bytes", stdio::MAX_ID),
          )
      };
      let Ok(Query(query)) = query else {
          return Err(Box::new(refused()));
      };
      match (query.host_id, query.hat_id) {
          (Some(host), Some(hat))
              if (1..=stdio::MAX_ID).contains(&host.len()) && (1..=stdio::MAX_ID).contains(&hat.len()) =>
          {
              Ok((host, hat))
          }
          _ => Err(Box::new(refused())),
      }
  }

  fn stdio_answer(host_id: String, hat_id: String, change: StdioChange) -> Response {
      match change {
          StdioChange::Done(servers) => Json(McpStdioServerSet {
              host_id,
              hat_id,
              servers: servers
                  .into_iter()
                  .map(|server| McpStdioServerItem {
                      name: server.name,
                      command: server.command,
                      args: server.args,
                      // Every name stored has its value stored (`""` too).
                      env: server
                          .env
                          .into_iter()
                          .map(|name| McpStdioEnvItem { name, has_value: true })
                          .collect(),
                      created_at: rfc3339(server.created_at),
                      updated_at: rfc3339(server.updated_at),
                  })
                  .collect(),
          })
          .into_response(),
          StdioChange::NotFound => error(StatusCode::NOT_FOUND, "not_found", "no such host or hat"),
          StdioChange::HostRevoked => error(
              StatusCode::CONFLICT,
              "host_revoked",
              "the host is revoked: its stdio servers can be read, not changed",
          ),
          StdioChange::Invalid(why) => error(StatusCode::BAD_REQUEST, "invalid", why),
          StdioChange::EnvValueMissing(why) => error(StatusCode::BAD_REQUEST, "env_value_missing", why),
          StdioChange::SlugTaken(why) => error(StatusCode::CONFLICT, "slug_taken", why),
          StdioChange::TooMany(why) => error(StatusCode::CONFLICT, "too_many_stdio_servers", why),
      }
  }

  /// `GET /api/mcp/stdio-servers?host_id=&hat_id=`: the set, without a value.
  async fn stdio_set(State(state): State<GatewayState>, query: Result<Query<StdioQuery>, QueryRejection>) -> Response {
      let (host_id, hat_id) = match stdio_place(query) {
          Ok(place) => place,
          Err(refused) => return *refused,
      };
      match state.store.stdio_set(&host_id, &hat_id) {
          Ok(change) => stdio_answer(host_id, hat_id, change),
          Err(err) => internal(err),
      }
  }

  /// `PUT /api/mcp/stdio-servers?host_id=&hat_id=` (step-up): the whole set,
  /// replaced. Logged by its host, hat and size only: a command's arguments
  /// and its values are the operator's secrets.
  async fn replace_stdio_set(
      State(state): State<GatewayState>,
      query: Result<Query<StdioQuery>, QueryRejection>,
      ApiJson(req): ApiJson<McpStdioServersRequest>,
  ) -> Response {
      let (host_id, hat_id) = match stdio_place(query) {
          Ok(place) => place,
          Err(refused) => return *refused,
      };
      let servers: Vec<StdioInput> = req
          .servers
          .into_iter()
          .map(|server| StdioInput {
              name: server.name,
              command: server.command,
              args: server.args,
              env: server.env.into_iter().map(|var| (var.name, var.value)).collect(),
          })
          .collect();
      match state
          .store
          .replace_stdio_set(&host_id, &hat_id, &servers, &state.key, unix_now())
      {
          Ok(change) => {
              if let StdioChange::Done(stored) = &change {
                  tracing::info!(%host_id, %hat_id, servers = stored.len(), "gateway stdio servers replaced");
              }
              stdio_answer(host_id, hat_id, change)
          }
          Err(err) => internal(err),
      }
  }
  ```

In `crates/hennery-gateway/src/crypto.rs`, replace:

  ```rust
  fn aad(connection_id: &str, field: &str, key_version: u32) -> Vec<u8> {
      let version = key_version.to_be_bytes();
      let mut out = Vec::with_capacity(12 + connection_id.len() + field.len() + version.len());
      for part in [connection_id.as_bytes(), field.as_bytes(), &version] {
  ```

with:

  ```rust
  /// The AAD's field for a stdio server's environment values (plan 8e): one
  /// sealed JSON object per `gw_stdio_servers` row.
  pub const STDIO_ENV: &str = "gw_stdio_servers.env";

  fn aad(connection_id: &str, field: &str, key_version: u32) -> Vec<u8> {
      aad_of(&[connection_id.as_bytes(), field.as_bytes(), &key_version.to_be_bytes()])
  }

  /// Each part preceded by its length as 4 bytes, big-endian. `aad`'s three
  /// parts are plan 8a's encoding, unchanged; a stdio server's (plan 8e, the
  /// API review's R5) are its row id, its host, its hat, the field and the
  /// version: five parts, so no stdio AAD reads as a connection's.
  fn aad_of(parts: &[&[u8]]) -> Vec<u8> {
      let mut out = Vec::with_capacity(parts.iter().map(|part| 4 + part.len()).sum());
      for part in parts {
  ```

In `crates/hennery-gateway/src/crypto.rs`, replace:

  ```rust
      }
      out
  }

  ```

with:

  ```rust
      }
      out
  }

  /// A stdio server row's AAD: bound to the row, its host and its hat (R5),
  /// so no `PUT` can move one hat's values into another hat or onto another
  /// host.
  fn stdio_aad(row_id: &str, host_id: &str, hat_id: &str, key_version: u32) -> Vec<u8> {
      aad_of(&[
          row_id.as_bytes(),
          host_id.as_bytes(),
          hat_id.as_bytes(),
          STDIO_ENV.as_bytes(),
          &key_version.to_be_bytes(),
      ])
  }

  ```

In `crates/hennery-gateway/src/crypto.rs`, replace:

  ```rust
      let nonce = hennery_kernel::secret::random_bytes::<NONCE_LEN>();
      let payload = Payload {
          msg: plaintext,
          aad: &aad(connection_id, field, key.version()),
      };
  ```

with:

  ```rust
      seal_with(key, &aad(connection_id, field, key.version()), plaintext)
  }

  /// Seal a stdio server row's environment values (`STDIO_ENV`).
  pub fn seal_stdio(key: &MasterKey, row_id: &str, host_id: &str, hat_id: &str, plaintext: &[u8]) -> Vec<u8> {
      seal_with(key, &stdio_aad(row_id, host_id, hat_id, key.version()), plaintext)
  }

  /// Open what `seal_stdio` stored for that row, host and hat.
  pub fn open_stdio(
      key: &MasterKey,
      row_id: &str,
      host_id: &str,
      hat_id: &str,
      key_version: u32,
      blob: &[u8],
  ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
      open_with(key, key_version, blob, |version| {
          stdio_aad(row_id, host_id, hat_id, version)
      })
  }

  fn seal_with(key: &MasterKey, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
      let nonce = hennery_kernel::secret::random_bytes::<NONCE_LEN>();
      let payload = Payload { msg: plaintext, aad };
  ```

In `crates/hennery-gateway/src/crypto.rs`, replace:

  ```rust
  ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
      if blob.len() < VERSION_LEN + NONCE_LEN + TAG_LEN {
  ```

with:

  ```rust
  ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
      open_with(key, key_version, blob, |version| aad(connection_id, field, version))
  }

  fn open_with(
      key: &MasterKey,
      key_version: u32,
      blob: &[u8],
      aad: impl FnOnce(u32) -> Vec<u8>,
  ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
      if blob.len() < VERSION_LEN + NONCE_LEN + TAG_LEN {
  ```

In `crates/hennery-gateway/src/crypto.rs`, replace:

  ```rust
          aad: &aad(connection_id, field, key_version),
  ```

with:

  ```rust
          aad: &aad(key_version),
  ```

In `crates/hennery-gateway/src/crypto.rs`, replace:

  ```rust
      }
  }
  ```

with:

  ```rust
      }

      /// Plan 8e: generalising the AAD left plan 8a's encoding byte for byte.
      #[test]
      fn a_connections_aad_is_plan_8as_encoding() {
          let mut expected = Vec::new();
          for part in [b"conn-1".as_slice(), STATIC_TOKEN.as_bytes(), &1u32.to_be_bytes()] {
              expected.extend_from_slice(&(part.len() as u32).to_be_bytes());
              expected.extend_from_slice(part);
          }
          assert_eq!(aad("conn-1", STATIC_TOKEN, 1), expected);
      }

      /// The API review's R5: a stdio row's values are bound to its row, its
      /// host and its hat; another of any does not open them, nor does a
      /// connection's field.
      #[test]
      fn a_stdio_blob_opens_only_for_its_row_host_and_hat() {
          let key = MasterKey::from_bytes([5; 32]);
          let blob = seal_stdio(&key, "stdio-1", "host-a", "hat-a", b"{}");
          assert_eq!(
              &*open_stdio(&key, "stdio-1", "host-a", "hat-a", 1, &blob).unwrap(),
              b"{}"
          );
          for (row, host, hat) in [
              ("stdio-2", "host-a", "hat-a"),
              ("stdio-1", "host-b", "hat-a"),
              ("stdio-1", "host-a", "hat-b"),
          ] {
              assert_eq!(
                  open_stdio(&key, row, host, hat, 1, &blob).unwrap_err(),
                  CryptoError::Refused,
                  "{row} {host} {hat}"
              );
          }
          assert_eq!(
              open(&key, "stdio-1", STATIC_TOKEN, 1, &blob).unwrap_err(),
              CryptoError::Refused
          );
      }
  }
  ```

In `crates/hennery-gateway/src/lib.rs`, replace:

  ```rust
  mod schema;
  pub mod scope;
  ```

with:

  ```rust
  pub mod revocation;
  mod schema;
  pub mod scope;
  pub mod session;
  pub mod stdio;
  ```

In `crates/hennery-gateway/src/lib.rs`, replace:

  ```rust
          operator,
      })
  ```

with:

  ```rust
          operator,
          revocations: revocation::Revocations::new(),
      })
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  use crate::jsonrpc::{self, Answered, BOM, EventOutcome, Inspected};
  use crate::key::MasterKey;
  use crate::model::{CredKind, url_for_logs};
  use crate::scope::{ClientIdentity, MountPolicy, Principal, ProxyStore, ScopedConnection};
  use crate::store::GatewayStore;
  use axum::Router;
  use axum::body::{Body, Bytes};
  ```

with:

  ```rust
  use crate::api::GatewayState;
  use crate::jsonrpc::{self, Answered, BOM, EventOutcome, Inspected};
  use crate::key::MasterKey;
  use crate::model::{CredKind, url_for_logs};
  use crate::revocation::{Revocations, Watch};
  use crate::scope::{ClientIdentity, MountPolicy, Principal, ProxyStore, ScopedConnection};
  use crate::store::GatewayStore;
  use crate::tokens::{is_session_token, token_hash};
  use axum::Router;
  use axum::body::{Body, Bytes, HttpBody};
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      pub limits: Limits,
  }
  ```

with:

  ```rust
      pub limits: Limits,
      /// What a revoke cuts (plan 8e decision 12): the same one the sessions'
      /// side revokes through (`session::GatewayMcp`).
      pub revocations: Revocations,
  }
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      /// `store`.
      pub fn full(
          store: Arc<ProxyStore>,
          credentials: Arc<GatewayStore>,
          key: Arc<MasterKey>,
          egress: Egress,
          limits: Limits,
      ) -> Self {
          Self {
              identity: store.clone(),
              mounts: store.clone(),
              credentials,
              statuses: store,
              key,
              egress,
              limits,
  ```

with:

  ```rust
      /// `store`; requests watched in `gateway`'s `Revocations`, with its
      /// store and key.
      pub fn full(store: Arc<ProxyStore>, gateway: &GatewayState, egress: Egress, limits: Limits) -> Self {
          Self {
              identity: store.clone(),
              mounts: store.clone(),
              credentials: gateway.store.clone(),
              statuses: store,
              key: gateway.key.clone(),
              egress,
              limits,
              revocations: gateway.revocations.clone(),
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  /// `POST|GET|DELETE /mcp/{slug}`.
  async fn proxy(
      State(state): State<ProxyState>,
  ```

with:

  ```rust
  /// `POST|GET|DELETE /mcp/{slug}`. A session token is watched from before
  /// it is resolved until the answer's body ends (plan 8e decision 12): a
  /// revoke that commits meanwhile ends the request, a 404 if no answer has
  /// begun, else its body cut short with an error. Bodies already whole in
  /// memory are left as they are.
  async fn proxy(
      State(state): State<ProxyState>,
      slug: Result<Path<String>, PathRejection>,
      method: Method,
      headers: HeaderMap,
      body: Body,
  ) -> Response {
      let watch = bearer(&headers)
          .filter(|token| is_session_token(token))
          .map(|token| state.revocations.watch(&token_hash(token)));
      let Some(watch) = watch else {
          // Never resolves: answered as for any unknown token.
          return forward(state, slug, method, headers, body).await;
      };
      let revoked = watch.token();
      let answer = tokio::select! {
          biased;
          () = revoked.cancelled() => {
              tracing::info!("gateway proxy: the session token was revoked before the answer");
              return not_found();
          }
          answer = forward(state, slug, method, headers, body) => answer,
      };
      if answer.body().size_hint().exact().is_some() {
          return answer;
      }
      let (parts, body) = answer.into_parts();
      Response::from_parts(parts, Body::from_stream(cut_off(body.into_data_stream(), watch)))
  }

  /// `body` until its token is cut, then an error, which ends the response
  /// mid-body (the client sees a broken stream, never a complete one), and
  /// drops the upstream's.
  fn cut_off<S>(body: S, watch: Watch) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static
  where
      S: Stream<Item = Result<Bytes, axum::Error>> + Send + 'static,
  {
      let revoked = watch.token();
      futures::stream::unfold(
          (Box::pin(body), Some(watch), revoked),
          |(mut body, watch, revoked)| async move {
              let watch = watch?;
              tokio::select! {
                  biased;
                  () = revoked.cancelled() => {
                      tracing::info!("gateway proxy: the session token was revoked, its stream cut");
                      Some((Err(std::io::Error::other("the session token was revoked")), (body, None, revoked)))
                  }
                  next = body.next() => match next {
                      Some(Ok(bytes)) => Some((Ok(bytes), (body, Some(watch), revoked))),
                      Some(Err(err)) => Some((Err(std::io::Error::other(err)), (body, None, revoked))),
                      None => None,
                  },
              }
          },
      )
  }

  /// The proxy's work for one request, once its token is watched.
  async fn forward(
      state: ProxyState,
  ```

Create `crates/hennery-gateway/src/revocation.rs`:

  ```rust
  //! A revoke ends what is open on the token (plan 8e decision 12, the
  //! fleet parent's ruling of 2026-10-02): scope that is checked only when a
  //! request arrives is not revocation. Every request the proxy serves on a
  //! session token watches that token here, from before the token is
  //! resolved until the answer's body ends; a revoke, once its transaction
  //! has committed, cuts every watch on the tokens it invalidated.
  //!
  //! The order closes the race with a revoke in flight: a revoke that
  //! commits before the proxy resolves the token makes the resolve fail (the
  //! same 404 as ever); one that commits after finds the watch already
  //! registered, and cuts it. Cutting before the commit would race a
  //! rollback, which would leave the token working with its streams cut.
  //!
  //! Tokens are named by their SHA-256 (`tokens::token_hash`), as stored: a
  //! supersession at resume cuts the old token's watches and not the new
  //! one's, which no request has presented yet.

  use std::collections::HashMap;
  use std::sync::{Arc, Mutex};
  use tokio_util::sync::CancellationToken;

  /// The tokens a transaction invalidated, by hash: revoked, superseded or
  /// deleted. Hand it to `Revocations::cut` once that transaction has
  /// committed; dropping it unused leaves their open streams running.
  #[must_use = "cut it after the transaction commits, or the revoked tokens' streams stay open"]
  #[derive(Debug, Default, PartialEq, Eq)]
  pub struct Cut(pub(crate) Vec<String>);

  impl Cut {
      pub fn is_empty(&self) -> bool {
          self.0.is_empty()
      }

      /// How many tokens it names.
      pub fn len(&self) -> usize {
          self.0.len()
      }

      /// This cut and `other`'s, for a transaction that invalidates in more
      /// than one place.
      pub fn and(mut self, other: Cut) -> Cut {
          self.0.extend(other.0);
          self
      }
  }

  #[derive(Default)]
  struct Entry {
      cancel: CancellationToken,
      watchers: usize,
  }

  /// Every open watch, by token hash. Cheap to clone: the proxy and the
  /// sessions' side of the gateway (`session::GatewayMcp`) share one.
  #[derive(Clone, Default)]
  pub struct Revocations {
      open: Arc<Mutex<HashMap<String, Entry>>>,
  }

  impl Revocations {
      pub fn new() -> Self {
          Self::default()
      }

      /// Watch the token with `hash` until the returned `Watch` is dropped.
      pub fn watch(&self, hash: &str) -> Watch {
          let mut open = self.open.lock().expect("revocations lock");
          let entry = open.entry(hash.to_string()).or_default();
          entry.watchers += 1;
          Watch {
              revocations: self.clone(),
              hash: hash.to_string(),
              cancel: entry.cancel.clone(),
          }
      }

      /// End every watch on the tokens `cut` names. Call it only after the
      /// transaction that invalidated them has committed.
      pub fn cut(&self, cut: Cut) {
          let mut open = self.open.lock().expect("revocations lock");
          for hash in cut.0 {
              if let Some(entry) = open.remove(&hash) {
                  entry.cancel.cancel();
              }
          }
      }

      /// How many tokens have an open watch (for tests).
      pub fn watched(&self) -> usize {
          self.open.lock().expect("revocations lock").len()
      }
  }

  /// One request's watch on its token.
  pub struct Watch {
      revocations: Revocations,
      hash: String,
      cancel: CancellationToken,
  }

  impl Watch {
      /// Cancelled when the token is cut.
      pub fn token(&self) -> CancellationToken {
          self.cancel.clone()
      }
  }

  impl Drop for Watch {
      fn drop(&mut self) {
          let mut open = self.revocations.open.lock().expect("revocations lock");
          // A cut watch's entry is gone already, and an entry for the same
          // hash now is a newer one's: left alone. An uncut watch's entry is
          // still its own (only a cut, or its last watch, removes one).
          if !self.cancel.is_cancelled()
              && let Some(entry) = open.get_mut(&self.hash)
          {
              entry.watchers -= 1;
              if entry.watchers == 0 {
                  open.remove(&self.hash);
              }
          }
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn a_cut_cancels_every_watch_on_its_tokens_and_no_other() {
          let revocations = Revocations::new();
          let a1 = revocations.watch("a");
          let a2 = revocations.watch("a");
          let b = revocations.watch("b");
          revocations.cut(Cut(vec!["a".into()]));
          assert!(a1.token().is_cancelled() && a2.token().is_cancelled());
          assert!(!b.token().is_cancelled());
      }

      #[test]
      fn a_watch_made_after_a_cut_is_not_cut_by_it() {
          let revocations = Revocations::new();
          let old = revocations.watch("a");
          revocations.cut(Cut(vec!["a".into()]));
          let new = revocations.watch("a");
          drop(old);
          assert!(!new.token().is_cancelled());
          assert_eq!(revocations.watched(), 1);
          drop(new);
          assert_eq!(revocations.watched(), 0);
      }

      #[test]
      fn the_last_watch_dropped_forgets_its_token() {
          let revocations = Revocations::new();
          let one = revocations.watch("a");
          let two = revocations.watch("a");
          drop(one);
          assert_eq!(revocations.watched(), 1);
          drop(two);
          assert_eq!(revocations.watched(), 0);
      }
  }
  ```

In `crates/hennery-gateway/src/schema.rs`, replace:

  ```rust
      ",
  ];
  ```

with:

  ```rust
      ",
      // Plan 8e: local stdio servers (gateway spec §3.4), one set per (host,
      // hat), replaced whole. A server keeps its row while its name stays
      // (decision E1). Only the environment's values are sealed (§2), as one
      // JSON object per row, bound to the row, its host and its hat
      // (`crypto::seal_stdio`, R5); their names are kept beside, in order.
      // A hat's sets go with its purge (`GatewayStore::purge_hat`), before
      // the hat row can be deleted; a revoked host's stay (decision E5).
      "
      CREATE TABLE gw_stdio_servers (
          id TEXT PRIMARY KEY,
          owner_id TEXT NOT NULL REFERENCES owners(id),
          host_id TEXT NOT NULL,
          hat_id TEXT NOT NULL,
          name TEXT NOT NULL,
          position INTEGER NOT NULL,
          command TEXT NOT NULL,
          args TEXT NOT NULL,
          env_names TEXT NOT NULL,
          key_version INTEGER,
          env_ciphertext BLOB,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          UNIQUE (owner_id, host_id, hat_id, name),
          CHECK ((key_version IS NULL) = (env_ciphertext IS NULL)),
          FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id),
          FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id));
      CREATE INDEX gw_stdio_servers_by_hat ON gw_stdio_servers(owner_id, hat_id);
      CREATE INDEX gw_stdio_servers_by_name ON gw_stdio_servers(owner_id, name);
      ",
  ];
  ```

Create `crates/hennery-gateway/src/session.rs`:

  ```rust
  //! What a session gets of the gateway (ACP core §1, gateway spec §3.2),
  //! through the one seam the sessions module reaches it by (umbrella §9):
  //! `SessionMcp`. The sessions store calls it inside the transactions of its
  //! own transitions (lane L1): a start's or resume's servers are read and
  //! its token minted before the transition commits, so a failed mint rolls
  //! it back; a park's, close's or revoke's token is revoked in that
  //! transition. What a transaction invalidated is cut (`Revocations`) only
  //! once it has committed (plan 8e decision 12).
  //!
  //! The delivery decision (umbrella §8.5) is the sessions module's (lane
  //! L2): it owns `sessions.hat_id`, and passes the decision in. This amends
  //! ACP core §1's `servers_for(host, hat, session)`.
  //!
  //! Every SQL statement here names the owner; the owner audit reads this
  //! file (`hennery-testkit/tests/owner_filter.rs`).

  use crate::api::GatewayState;
  use crate::key::MasterKey;
  use crate::revocation::{Cut, Revocations};
  use crate::store::GatewayStore;
  use crate::{stdio, tokens};
  use anyhow::{Result, anyhow};
  use hennery_kernel::operator::Operator;
  use hennery_kernel::secret::unix_now;
  use hennery_proto::frames::{McpServer, NameValue};
  use hennery_proto::rest::McpSessionDeliveryMode;
  use rusqlite::{Transaction, params};
  use std::sync::Arc;

  /// The session a start or resume is for, as its row says.
  #[derive(Debug, Clone, Copy)]
  pub struct SessionRef<'a> {
      pub session_id: &'a str,
      pub host_id: &'a str,
      pub hat_id: &'a str,
  }

  /// What a start or resume gets: its servers, and the token it superseded
  /// or revoked, to cut once the transaction commits.
  #[derive(Debug)]
  pub struct Delivered {
      /// `hennery-<slug>` HTTP entries first (the hat's connections mounted
      /// on the host, each with the session's token), then the stdio
      /// servers. Their values are secrets: they go into the session's frame
      /// and nowhere else.
      pub servers: Vec<McpServer>,
      pub cut: Cut,
  }

  /// The gateway, as the sessions module sees it (umbrella §9, ACP core §1).
  pub trait SessionMcp: Send + Sync {
      /// The servers of `session`'s start or resume, inside its transaction:
      /// with a delivering `mode`, the hat's connections mounted on the host,
      /// with a fresh token minted (superseding the session's previous one),
      /// then the hat's stdio servers on that host. With any other mode none,
      /// and the previous token revoked. An error should roll the transition
      /// back.
      fn servers_in(
          &self,
          tx: &Transaction<'_>,
          session: SessionRef<'_>,
          mode: McpSessionDeliveryMode,
      ) -> Result<Delivered>;

      /// Revoke the session's token inside the caller's transaction (lane
      /// L4's sites). Revoking twice, or a session that never had one, is
      /// not an error.
      fn revoke_in(&self, tx: &Transaction<'_>, session_id: &str) -> Result<Cut>;

      /// Revoke every live token of a host (a host revoke, ACP core §4.8).
      fn revoke_host_in(&self, tx: &Transaction<'_>, host_id: &str) -> Result<Cut>;

      /// End what is open on the tokens `cut` names. Only after the
      /// transaction that produced it has committed.
      fn cut(&self, cut: Cut);

      /// The gateway's part of a hat's purge (kernel spec §5.5, lane L6), in
      /// its own transaction, cut once it commits. Idempotent.
      fn purge_hat(&self, hat_id: &str) -> Result<()>;
  }

  /// No gateway: no servers, nothing to revoke. A sessions store opened on
  /// its own (its tests) has this until the collector gives it the gateway.
  pub struct NoSessionMcp;

  impl SessionMcp for NoSessionMcp {
      fn servers_in(&self, _: &Transaction<'_>, _: SessionRef<'_>, _: McpSessionDeliveryMode) -> Result<Delivered> {
          Ok(Delivered {
              servers: Vec::new(),
              cut: Cut::default(),
          })
      }

      fn revoke_in(&self, _: &Transaction<'_>, _: &str) -> Result<Cut> {
          Ok(Cut::default())
      }

      fn revoke_host_in(&self, _: &Transaction<'_>, _: &str) -> Result<Cut> {
          Ok(Cut::default())
      }

      fn cut(&self, _: Cut) {}

      fn purge_hat(&self, _: &str) -> Result<()> {
          Ok(())
      }
  }

  /// The collector's gateway, on the same `hennery.db` as the sessions
  /// store, whose transactions it is handed.
  pub struct GatewayMcp {
      store: Arc<GatewayStore>,
      key: Arc<MasterKey>,
      operator: Arc<Operator>,
      revocations: Revocations,
  }

  impl GatewayMcp {
      /// On `gateway`'s store, key and owner, cutting through the same
      /// `Revocations` its proxy watches.
      pub fn new(gateway: &GatewayState) -> Self {
          Self {
              store: gateway.store.clone(),
              key: gateway.key.clone(),
              operator: gateway.operator.clone(),
              revocations: gateway.revocations.clone(),
          }
      }

      fn owner(&self) -> &str {
          self.store.owner_id()
      }
  }

  /// The slugs of `hat_id`'s connections mounted on `host_id`, a host that is
  /// not revoked: what `MountPolicy::connection` would let the session's
  /// token reach, with the same joins. Oldest first.
  fn mounted_slugs_in(tx: &Transaction<'_>, owner: &str, host_id: &str, hat_id: &str) -> Result<Vec<String>> {
      let mut stmt = tx.prepare(
          "SELECT c.slug FROM gw_connections c
           WHERE c.owner_id = ?1 AND c.hat_id = ?2
               AND EXISTS (SELECT 1 FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
                           WHERE m.connection_id = c.id AND m.owner_id = ?1 AND m.host_id = ?3
                               AND h.revoked_at IS NULL)
           ORDER BY c.slug",
      )?;
      let rows = stmt.query_map(params![owner, hat_id, host_id], |r| r.get(0))?;
      Ok(rows.collect::<rusqlite::Result<_>>()?)
  }

  impl SessionMcp for GatewayMcp {
      fn servers_in(
          &self,
          tx: &Transaction<'_>,
          session: SessionRef<'_>,
          mode: McpSessionDeliveryMode,
      ) -> Result<Delivered> {
          let owner = self.owner();
          let now = unix_now();
          // Whatever this start or resume gets, the token before it is done.
          let cut = Cut(tokens::session_hash_in(tx, owner, session.session_id)?
              .into_iter()
              .collect());
          // Nothing to deliver; the mode decides. (A session from before hats,
          // hat "", is never given a server: no connection or stdio server
          // can name that hat, their foreign keys refuse it.)
          if !mode.delivers() {
              tokens::revoke_in(tx, owner, session.session_id, now)?;
              return Ok(Delivered {
                  servers: Vec::new(),
                  cut,
              });
          }
          let slugs = mounted_slugs_in(tx, owner, session.host_id, session.hat_id)?;
          let mut servers = Vec::with_capacity(slugs.len());
          if slugs.is_empty() {
              // No connection to reach: no token at all.
              tokens::revoke_in(tx, owner, session.session_id, now)?;
          } else {
              let public = self
                  .operator
                  .public_url()
                  .ok_or_else(|| anyhow!("no public_url to name the gateway's /mcp/ by"))?;
              let token = tokens::mint_in(tx, owner, session.session_id, session.host_id, session.hat_id, now)?;
              for slug in slugs {
                  servers.push(McpServer::Http {
                      name: format!("hennery-{slug}"),
                      url: format!("{}/mcp/{slug}", public.origin()),
                      headers: vec![NameValue::new("Authorization", format!("Bearer {}", token.expose()))],
                  });
              }
          }
          servers.extend(stdio::delivered_in(
              tx,
              owner,
              session.host_id,
              session.hat_id,
              &self.key,
          )?);
          Ok(Delivered { servers, cut })
      }

      fn revoke_in(&self, tx: &Transaction<'_>, session_id: &str) -> Result<Cut> {
          let owner = self.owner();
          let hash = tokens::session_hash_in(tx, owner, session_id)?;
          let revoked = tokens::revoke_in(tx, owner, session_id, unix_now())?;
          // Only a live token has anything open to end.
          Ok(Cut(hash.filter(|_| revoked).into_iter().collect()))
      }

      fn revoke_host_in(&self, tx: &Transaction<'_>, host_id: &str) -> Result<Cut> {
          let owner = self.owner();
          let cut = Cut(tokens::live_host_hashes_in(tx, owner, host_id)?);
          tokens::revoke_host_in(tx, owner, host_id, unix_now())?;
          Ok(cut)
      }

      fn cut(&self, cut: Cut) {
          self.revocations.cut(cut);
      }

      fn purge_hat(&self, hat_id: &str) -> Result<()> {
          let cut = self.store.purge_hat(hat_id)?;
          self.revocations.cut(cut);
          Ok(())
      }
  }
  ```

Create `crates/hennery-gateway/src/stdio.rs`:

  ```rust
  //! Local stdio servers (gateway spec §3.4; plan 8e decisions E1–E6): one
  //! set per (host, hat), replaced whole, passed to that hat's sessions on
  //! that host as ACP stdio `mcpServers` entries named `hennery-<name>`. The
  //! agent runs them on the host; the gateway never proxies them.
  //!
  //! Only the environment's values are sealed (§2, §6): one JSON object per
  //! row, under `crypto::seal_stdio`, bound to the row, its host and its hat
  //! (the API review's R5). Their names are kept beside them, in order, so a
  //! `GET` reads no secret. Every SQL statement here names the owner, and the
  //! owner audit reads this file (`hennery-testkit/tests/owner_filter.rs`).

  use crate::crypto;
  use crate::key::MasterKey;
  use crate::store::GatewayStore;
  use anyhow::{Context, Result, anyhow};
  use hennery_proto::frames::{McpServer, NameValue};
  use rusqlite::{Connection, OptionalExtension, Transaction, params};
  use std::collections::{BTreeMap, HashMap, HashSet};
  use zeroize::Zeroizing;

  /// The most servers in one (host, hat)'s set.
  pub const MAX_SET: usize = 32;
  /// The most servers one owner has, across sets.
  pub const MAX_OWNER: usize = 1024;
  /// A command's most bytes.
  pub const MAX_COMMAND: usize = 1024;
  /// The most arguments, each one's most bytes, and all of them together.
  pub const MAX_ARGS: usize = 64;
  pub const MAX_ARG: usize = 4096;
  pub const MAX_ARGS_TOTAL: usize = 16 * 1024;
  /// The most environment variables per server, and a value's most bytes.
  pub const MAX_ENV: usize = 64;
  pub const MAX_ENV_VALUE: usize = 8192;
  /// The longest host or hat id a query names, checked before any read.
  pub const MAX_ID: usize = 64;

  /// One server as stored, without its values.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct StdioServer {
      pub name: String,
      pub command: String,
      pub args: Vec<String>,
      /// The environment's names, in order; each has a value stored.
      pub env: Vec<String>,
      pub created_at: i64,
      pub updated_at: i64,
  }

  /// One server of a `PUT`. A `value` of `None` keeps the stored one.
  #[derive(Clone, PartialEq, Eq)]
  pub struct StdioInput {
      pub name: String,
      pub command: String,
      pub args: Vec<String>,
      pub env: Vec<(String, Option<String>)>,
  }

  // By hand: the args and the values may be secrets.
  impl std::fmt::Debug for StdioInput {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("StdioInput")
              .field("name", &self.name)
              .field("command", &self.command)
              .finish_non_exhaustive()
      }
  }

  /// The outcome of `GatewayStore::stdio_set` and `replace_stdio_set`.
  #[derive(Debug, PartialEq, Eq)]
  pub enum StdioChange {
      /// The set as stored now, oldest first.
      Done(Vec<StdioServer>),
      /// Not one of the owner's hosts, or hats (a hat being purged is none).
      NotFound,
      /// The host is revoked: its set can be read, not changed (E5).
      HostRevoked,
      /// A server or field refused; the message names which, never a value.
      Invalid(String),
      /// A kept value that is not stored, or whose server's command changed.
      EnvValueMissing(String),
      /// A name one of the owner's connections has as its slug (E2).
      SlugTaken(String),
      /// More than `MAX_SET` in the set, or `MAX_OWNER` for the owner.
      TooMany(String),
  }

  /// `^[a-z0-9][a-z0-9-]{0,47}$`, as a connection's slug (E2).
  fn name_ok(name: &str) -> bool {
      let bytes = name.as_bytes();
      (1..=48).contains(&bytes.len())
          && bytes[0] != b'-'
          && bytes
              .iter()
              .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
  }

  /// `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
  fn env_name_ok(name: &str) -> bool {
      let bytes = name.as_bytes();
      (1..=128).contains(&bytes.len())
          && !bytes[0].is_ascii_digit()
          && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
  }

  /// The first rule `servers` breaks, if any, before anything is read: what
  /// is refused 400 `invalid` or 409 `too_many_stdio_servers`.
  fn problem(servers: &[StdioInput]) -> Option<StdioChange> {
      if servers.len() > MAX_SET {
          return Some(StdioChange::TooMany(format!(
              "a host and hat have at most {MAX_SET} stdio servers"
          )));
      }
      let mut names = HashSet::new();
      for (i, server) in servers.iter().enumerate() {
          if !name_ok(&server.name) {
              return Some(StdioChange::Invalid(format!(
                  "server {}: a name is 1 to 48 of a-z, 0-9 and -, not starting with -",
                  i + 1
              )));
          }
          let at = &server.name;
          if !names.insert(at.as_str()) {
              return Some(StdioChange::Invalid(format!("server {at:?} is named twice")));
          }
          if server.command.is_empty()
              || server.command.len() > MAX_COMMAND
              || server.command.chars().any(char::is_control)
          {
              return Some(StdioChange::Invalid(format!(
                  "server {at:?}: the command is 1 to {MAX_COMMAND} bytes, with no control characters"
              )));
          }
          if server.args.len() > MAX_ARGS
              || server.args.iter().any(|a| a.len() > MAX_ARG || a.contains('\0'))
              || server.args.iter().map(String::len).sum::<usize>() > MAX_ARGS_TOTAL
          {
              return Some(StdioChange::Invalid(format!(
                  "server {at:?}: the args are at most {MAX_ARGS}, each at most {MAX_ARG} bytes with no NUL, \
                   {MAX_ARGS_TOTAL} bytes in all"
              )));
          }
          if server.env.len() > MAX_ENV {
              return Some(StdioChange::Invalid(format!(
                  "server {at:?}: the env has at most {MAX_ENV} variables"
              )));
          }
          let mut env_names = HashSet::new();
          for (j, (name, value)) in server.env.iter().enumerate() {
              if !env_name_ok(name) {
                  return Some(StdioChange::Invalid(format!(
                      "server {at:?}: env variable {} is not named as ^[A-Za-z_][A-Za-z0-9_]{{0,127}}$",
                      j + 1
                  )));
              }
              if !env_names.insert(name.as_str()) {
                  return Some(StdioChange::Invalid(format!(
                      "server {at:?}: env variable {name} is named twice"
                  )));
              }
              if value
                  .as_ref()
                  .is_some_and(|v| v.len() > MAX_ENV_VALUE || v.contains('\0'))
              {
                  return Some(StdioChange::Invalid(format!(
                      "server {at:?}: the value of {name} is at most {MAX_ENV_VALUE} bytes, with no NUL"
                  )));
              }
          }
      }
      None
  }

  /// A stored row: its id and what a kept value needs.
  struct Row {
      id: String,
      command: String,
      args: Vec<String>,
      env: Vec<String>,
      key_version: Option<u32>,
      ciphertext: Option<Vec<u8>>,
      created_at: i64,
      updated_at: i64,
  }

  const SELECT_SET: &str = "SELECT id, name, command, args, env_names, key_version, env_ciphertext, created_at,
              updated_at
       FROM gw_stdio_servers
       WHERE owner_id = ?1 AND host_id = ?2 AND hat_id = ?3
       ORDER BY created_at, position, id";

  /// The set's rows, oldest first, by name.
  fn rows(conn: &Connection, owner: &str, host_id: &str, hat_id: &str) -> Result<Vec<(String, Row)>> {
      let mut stmt = conn.prepare(SELECT_SET)?;
      let rows = stmt.query_map(params![owner, host_id, hat_id], |r| {
          Ok((
              r.get::<_, String>(1)?,
              Row {
                  id: r.get(0)?,
                  command: r.get(2)?,
                  args: Vec::new(),
                  env: Vec::new(),
                  key_version: r.get(5)?,
                  ciphertext: r.get(6)?,
                  created_at: r.get(7)?,
                  updated_at: r.get(8)?,
              },
              r.get::<_, String>(3)?,
              r.get::<_, String>(4)?,
          ))
      })?;
      let mut out = Vec::new();
      for row in rows {
          let (name, mut row, args, env) = row?;
          row.args = serde_json::from_str(&args).context("a stored stdio server's args")?;
          row.env = serde_json::from_str(&env).context("a stored stdio server's env names")?;
          out.push((name, row));
      }
      Ok(out)
  }

  fn listed(rows: Vec<(String, Row)>) -> Vec<StdioServer> {
      rows.into_iter()
          .map(|(name, row)| StdioServer {
              name,
              command: row.command,
              args: row.args,
              env: row.env,
              created_at: row.created_at,
              updated_at: row.updated_at,
          })
          .collect()
  }

  /// A row's values, opened. An error if they do not open (another key, a
  /// blob moved from another row, host or hat) or do not read.
  fn values(key: &MasterKey, host_id: &str, hat_id: &str, row: &Row) -> Result<BTreeMap<String, Zeroizing<String>>> {
      let (Some(version), Some(blob)) = (row.key_version, row.ciphertext.as_deref()) else {
          return Ok(BTreeMap::new());
      };
      let opened = crypto::open_stdio(key, &row.id, host_id, hat_id, version, blob)
          .with_context(|| format!("stdio server {}", row.id))?;
      let parsed: BTreeMap<String, String> =
          serde_json::from_slice(&opened).map_err(|_| anyhow!("stdio server {}: its values do not read", row.id))?;
      Ok(parsed.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect())
  }

  /// Whether the host and the hat are the owner's, and the host revoked.
  /// A hat being purged (`purged_hats`) is no hat here: its sets go with
  /// the purge.
  fn place(conn: &Connection, owner: &str, host_id: &str, hat_id: &str) -> Result<Option<bool>> {
      let hat = conn
          .query_row(
              "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2
                   AND NOT EXISTS (SELECT 1 FROM purged_hats WHERE hat_id = ?1 AND owner_id = ?2)",
              [hat_id, owner],
              |_| Ok(()),
          )
          .optional()?;
      if hat.is_none() {
          return Ok(None);
      }
      let revoked: Option<Option<i64>> = conn
          .query_row(
              "SELECT revoked_at FROM hosts WHERE id = ?1 AND owner_id = ?2",
              [host_id, owner],
              |r| r.get(0),
          )
          .optional()?;
      Ok(revoked.map(|at| at.is_some()))
  }

  impl GatewayStore {
      /// The (host, hat)'s set, oldest first; a revoked host's too.
      pub fn stdio_set(&self, host_id: &str, hat_id: &str) -> Result<StdioChange> {
          let conn = self.conn();
          if place(&conn, self.owner_id(), host_id, hat_id)?.is_none() {
              return Ok(StdioChange::NotFound);
          }
          Ok(StdioChange::Done(listed(rows(
              &conn,
              self.owner_id(),
              host_id,
              hat_id,
          )?)))
      }

      /// Replace the (host, hat)'s set with `servers` (gateway spec §3.4;
      /// decisions E1–E5), in one transaction: a server keeps its row while
      /// its name stays; a value left `None` is kept from the same row, and
      /// only while its command is unchanged (S1); a name that is one of the
      /// owner's connection slugs is refused. A refused set changes nothing.
      pub fn replace_stdio_set(
          &self,
          host_id: &str,
          hat_id: &str,
          servers: &[StdioInput],
          key: &MasterKey,
          now: i64,
      ) -> Result<StdioChange> {
          if let Some(refused) = problem(servers) {
              return Ok(refused);
          }
          let owner = self.owner_id().to_string();
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          match place(&tx, &owner, host_id, hat_id)? {
              None => return Ok(StdioChange::NotFound),
              Some(true) => return Ok(StdioChange::HostRevoked),
              Some(false) => {}
          }
          let stored: HashMap<String, Row> = rows(&tx, &owner, host_id, hat_id)?.into_iter().collect();
          // Every value first: a refusal must change nothing.
          let mut sealed = Vec::with_capacity(servers.len());
          for server in servers {
              let before = stored.get(&server.name);
              let mut kept: Option<BTreeMap<String, Zeroizing<String>>> = None;
              let mut env: BTreeMap<String, Zeroizing<String>> = BTreeMap::new();
              for (name, value) in &server.env {
                  let value = match value {
                      Some(value) => Zeroizing::new(value.clone()),
                      None => {
                          let missing = || {
                              StdioChange::EnvValueMissing(format!(
                                  "server {:?}: no value of {name} is kept: send it",
                                  server.name
                              ))
                          };
                          let Some(before) = before.filter(|b| b.command == server.command) else {
                              return Ok(missing());
                          };
                          if kept.is_none() {
                              kept = Some(values(key, host_id, hat_id, before)?);
                          }
                          match kept.as_ref().and_then(|values| values.get(name)) {
                              Some(value) => value.clone(),
                              None => return Ok(missing()),
                          }
                      }
                  };
                  env.insert(name.clone(), value);
              }
              sealed.push(env);
          }
          let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
          for name in &names {
              let taken = tx
                  .query_row(
                      "SELECT 1 FROM gw_connections WHERE slug = ?1 AND owner_id = ?2",
                      [name, &owner.as_str()],
                      |_| Ok(()),
                  )
                  .optional()?;
              if taken.is_some() {
                  return Ok(StdioChange::SlugTaken(format!(
                      "{name:?} is one of the connections' slugs"
                  )));
              }
          }
          let elsewhere: i64 = tx.query_row(
              "SELECT count(*) FROM gw_stdio_servers WHERE owner_id = ?1 AND NOT (host_id = ?2 AND hat_id = ?3)",
              params![owner, host_id, hat_id],
              |r| r.get(0),
          )?;
          if elsewhere as usize + servers.len() > MAX_OWNER {
              return Ok(StdioChange::TooMany(format!(
                  "there are at most {MAX_OWNER} stdio servers"
              )));
          }
          for (name, row) in &stored {
              if !names.contains(&name.as_str()) {
                  tx.execute(
                      "DELETE FROM gw_stdio_servers WHERE id = ?1 AND owner_id = ?2",
                      [&row.id, &owner],
                  )?;
              }
          }
          for (position, (server, env)) in servers.iter().zip(sealed).enumerate() {
              let before = stored.get(&server.name);
              let id = before.map_or_else(
                  || format!("stdio-{}", hex::encode(hennery_kernel::secret::random_bytes::<8>())),
                  |row| row.id.clone(),
              );
              let env_names: Vec<&str> = server.env.iter().map(|(name, _)| name.as_str()).collect();
              let (version, blob) = if env.is_empty() {
                  (None, None)
              } else {
                  let plain: BTreeMap<&str, &str> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                  let plain = Zeroizing::new(serde_json::to_vec(&plain)?);
                  (
                      Some(key.version()),
                      Some(crypto::seal_stdio(key, &id, host_id, hat_id, &plain)),
                  )
              };
              let changed = before.is_none_or(|b| {
                  b.command != server.command
                      || b.args != server.args
                      || b.env.iter().map(String::as_str).ne(env_names.iter().copied())
                      || server.env.iter().any(|(_, value)| value.is_some())
              });
              tx.execute(
                  "INSERT INTO gw_stdio_servers(id, owner_id, host_id, hat_id, name, position, command, args,
                                                env_names, key_version, env_ciphertext, created_at, updated_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
                   ON CONFLICT(id) DO UPDATE SET position = excluded.position, command = excluded.command,
                       args = excluded.args, env_names = excluded.env_names, key_version = excluded.key_version,
                       env_ciphertext = excluded.env_ciphertext,
                       updated_at = CASE WHEN ?13 THEN excluded.updated_at ELSE gw_stdio_servers.updated_at END
                       WHERE gw_stdio_servers.owner_id = excluded.owner_id",
                  params![
                      id,
                      owner,
                      host_id,
                      hat_id,
                      server.name,
                      position as i64,
                      server.command,
                      serde_json::to_string(&server.args)?,
                      serde_json::to_string(&env_names)?,
                      version,
                      blob,
                      now,
                      changed,
                  ],
              )?;
          }
          let done = listed(rows(&tx, &owner, host_id, hat_id)?);
          tx.commit()?;
          Ok(StdioChange::Done(done))
      }
  }

  /// The (host, hat)'s stdio servers as a session gets them, inside the
  /// sessions store's transaction (lane L1): `hennery-<name>`, the command
  /// and args as stored, the values opened. Oldest first.
  pub(crate) fn delivered_in(
      tx: &Transaction<'_>,
      owner: &str,
      host_id: &str,
      hat_id: &str,
      key: &MasterKey,
  ) -> Result<Vec<McpServer>> {
      let mut out = Vec::new();
      for (name, row) in rows(tx, owner, host_id, hat_id)? {
          let values = values(key, host_id, hat_id, &row)?;
          let mut env = Vec::with_capacity(row.env.len());
          for var in &row.env {
              let value = values
                  .get(var)
                  .ok_or_else(|| anyhow!("stdio server {}: no value of {var} is stored", row.id))?;
              env.push(NameValue::new(var.as_str(), value.as_str()));
          }
          out.push(McpServer::Stdio {
              name: format!("hennery-{name}"),
              command: row.command,
              args: row.args,
              env,
          });
      }
      Ok(out)
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      fn server(name: &str) -> StdioInput {
          StdioInput {
              name: name.into(),
              command: "files-mcp".into(),
              args: vec![],
              env: vec![],
          }
      }

      fn invalid(servers: &[StdioInput]) -> String {
          match problem(servers) {
              Some(StdioChange::Invalid(why)) => why,
              other => panic!("{other:?}"),
          }
      }

      #[test]
      fn every_limit_is_refused_and_names_no_value() {
          assert!(problem(&[server("files")]).is_none());
          let secret = "s3cr3t-value";
          let mut bad = server("files");
          bad.command = String::new();
          assert!(invalid(&[bad]).contains("command"));
          let mut bad = server("files");
          bad.command = "a\u{7}b".into();
          assert!(invalid(&[bad]).contains("command"));
          let mut bad = server("files");
          bad.args = vec![format!("{secret}\0")];
          assert!(!invalid(&[bad]).contains(secret));
          let mut bad = server("files");
          bad.args = vec!["x".into(); MAX_ARGS + 1];
          assert!(invalid(&[bad]).contains("args"));
          let mut bad = server("files");
          bad.args = vec!["x".repeat(MAX_ARG); 5];
          assert!(invalid(&[bad]).contains("args"));
          let mut bad = server("files");
          bad.env = (0..=MAX_ENV).map(|i| (format!("V{i}"), Some(String::new()))).collect();
          assert!(invalid(&[bad]).contains("env"));
          let mut bad = server("files");
          bad.env = vec![("1BAD".into(), None)];
          assert!(invalid(&[bad]).contains("env variable 1"));
          let mut bad = server("files");
          bad.env = vec![("A".into(), None), ("A".into(), None)];
          assert!(invalid(&[bad]).contains("twice"));
          let mut bad = server("files");
          bad.env = vec![("A".into(), Some(format!("{secret}\0")))];
          let why = invalid(&[bad]);
          assert!(why.contains("value of A") && !why.contains(secret), "{why}");
          assert!(invalid(&[server("-files")]).contains("server 1"));
          assert!(invalid(&[server("Files")]).contains("server 1"));
          assert!(invalid(&[server("files"), server("files")]).contains("twice"));
          let many: Vec<StdioInput> = (0..=MAX_SET).map(|i| server(&format!("s{i}"))).collect();
          assert!(matches!(problem(&many), Some(StdioChange::TooMany(_))));
      }

      #[test]
      fn env_names_follow_the_shell_rule() {
          for good in ["A", "_", "a_1", &"A".repeat(128)] {
              assert!(env_name_ok(good), "{good}");
          }
          for bad in ["", "1A", "A-B", "A B", &"A".repeat(129)] {
              assert!(!env_name_ok(bad), "{bad}");
          }
      }
  }
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
  use crate::schema::{COMPONENT, MIGRATIONS};
  ```

with:

  ```rust
  use crate::revocation::Cut;
  use crate::schema::{COMPONENT, MIGRATIONS};
  use crate::tokens;
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
      fn conn(&self) -> MutexGuard<'_, Connection> {
  ```

with:

  ```rust
      /// The store's connection: `stdio.rs` and `session.rs` hold their
      /// statements on it, which the owner audit reads too.
      pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
          let taken = tx
              .query_row(
                  "SELECT 1 FROM gw_connections WHERE slug = ?1 AND owner_id = ?2",
  ```

with:

  ```rust
          // A stdio server's name too (plan 8e decision E2): both are
          // `hennery-<name>` to an agent. In this transaction, as the insert.
          let taken = tx
              .query_row(
                  "SELECT 1 FROM gw_connections WHERE slug = ?1 AND owner_id = ?2
                   UNION ALL SELECT 1 FROM gw_stdio_servers WHERE name = ?1 AND owner_id = ?2",
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
          Ok(self.conn().query_row(
              "SELECT EXISTS (SELECT 1 FROM gw_credentials WHERE owner_id = ?1)",
  ```

with:

  ```rust
          // A stdio server's sealed values too (plan 8a's hand-off to 8e).
          Ok(self.conn().query_row(
              "SELECT EXISTS (SELECT 1 FROM gw_credentials WHERE owner_id = ?1)
                   OR EXISTS (SELECT 1 FROM gw_stdio_servers WHERE owner_id = ?1 AND env_ciphertext IS NOT NULL)",
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
          let Some((id, version, blob, kind)) = row else {
              return Ok(());
          };
          anyhow::ensure!(
              kind == CredKind::Static.as_str(),
              "connection {id} has a credential of kind {kind}"
          );
          crypto::open(key, &id, STATIC_TOKEN, version, &blob)
              .map(drop)
              .with_context(|| {
                  format!(
                      "the master key does not open the stored credential of connection {id}: restore the key it was \
  ```

with:

  ```rust
          if let Some((id, version, blob, kind)) = row {
              anyhow::ensure!(
                  kind == CredKind::Static.as_str(),
                  "connection {id} has a credential of kind {kind}"
              );
              crypto::open(key, &id, STATIC_TOKEN, version, &blob)
                  .map(drop)
                  .with_context(|| {
                      format!(
                          "the master key does not open the stored credential of connection {id}: restore the key it \
                           was sealed with. {}",
                          crate::key::GIVE_UP
                      )
                  })?;
          }
          // And the newest stdio server's values (plan 8e), likewise.
          let row: Option<(String, String, String, u32, Vec<u8>)> = self
              .conn()
              .query_row(
                  "SELECT id, host_id, hat_id, key_version, env_ciphertext FROM gw_stdio_servers
                   WHERE owner_id = ?1 AND env_ciphertext IS NOT NULL
                   ORDER BY updated_at DESC, id LIMIT 1",
                  [&self.owner],
                  |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
              )
              .optional()?;
          let Some((id, host_id, hat_id, version, blob)) = row else {
              return Ok(());
          };
          crypto::open_stdio(key, &id, &host_id, &hat_id, version, &blob)
              .map(drop)
              .with_context(|| {
                  format!(
                      "the master key does not open the stored values of stdio server {id}: restore the key they were \
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
      /// hat's connections, with their credentials and mounts, in one
      /// transaction. Idempotent: a hat with nothing left, or gone, is done.
      pub fn purge_hat(&self, hat_id: &str) -> Result<()> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
  ```

with:

  ```rust
      /// hat's session tokens (`tokens::purge_hat_in`, plan 8d's hand-off),
      /// its stdio servers (plan 8e), and its connections with their
      /// credentials and mounts, in one transaction. Idempotent: a hat with
      /// nothing left, or gone, is done. The tokens it deleted are to be cut
      /// once it returns (`Revocations::cut`).
      pub fn purge_hat(&self, hat_id: &str) -> Result<Cut> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let cut = Cut(tokens::hat_hashes_in(&tx, &self.owner, hat_id)?);
          tokens::purge_hat_in(&tx, &self.owner, hat_id)?;
          tx.execute(
              "DELETE FROM gw_stdio_servers WHERE hat_id = ?1 AND owner_id = ?2",
              [hat_id, &self.owner],
          )?;
  ```

In `crates/hennery-gateway/src/store.rs`, replace:

  ```rust
          Ok(())
  ```

with:

  ```rust
          Ok(cut)
  ```

In `crates/hennery-gateway/src/tokens.rs`, replace:

  ```rust
  use rusqlite::{Transaction, params};
  ```

with:

  ```rust
  use rusqlite::{OptionalExtension, Transaction, params};
  ```

In `crates/hennery-gateway/src/tokens.rs`, replace:

  ```rust

  #[cfg(test)]
  ```

with:

  ```rust

  /// The hash of `session_id`'s token, revoked or not: what a mint
  /// supersedes and a revoke ends (`revocation::Cut`).
  pub(crate) fn session_hash_in(tx: &Transaction<'_>, owner_id: &str, session_id: &str) -> Result<Option<String>> {
      Ok(tx
          .query_row(
              "SELECT token_hash FROM gw_session_tokens WHERE session_id = ?1 AND owner_id = ?2",
              params![session_id, owner_id],
              |r| r.get(0),
          )
          .optional()?)
  }

  /// The hashes of `host_id`'s live tokens, before `revoke_host_in`.
  pub(crate) fn live_host_hashes_in(tx: &Transaction<'_>, owner_id: &str, host_id: &str) -> Result<Vec<String>> {
      let mut stmt = tx.prepare(
          "SELECT token_hash FROM gw_session_tokens WHERE host_id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
      )?;
      let rows = stmt.query_map(params![host_id, owner_id], |r| r.get(0))?;
      Ok(rows.collect::<rusqlite::Result<_>>()?)
  }

  /// The hashes of `hat_id`'s tokens, before `purge_hat_in`.
  pub(crate) fn hat_hashes_in(tx: &Transaction<'_>, owner_id: &str, hat_id: &str) -> Result<Vec<String>> {
      let mut stmt = tx.prepare("SELECT token_hash FROM gw_session_tokens WHERE hat_id = ?1 AND owner_id = ?2")?;
      let rows = stmt.query_map(params![hat_id, owner_id], |r| r.get(0))?;
      Ok(rows.collect::<rusqlite::Result<_>>()?)
  }

  #[cfg(test)]
  ```

In `crates/hennery-kernel/src/capabilities.rs`, replace:

  ```rust
  /// served.
  pub fn features() -> Vec<String> {
      vec!["mcp_connections".to_owned()]
  ```

with:

  ```rust
  /// served, and plan 8e's stdio servers (`/api/mcp/stdio-servers`).
  pub fn features() -> Vec<String> {
      vec!["mcp_connections".to_owned(), "mcp_stdio".to_owned()]
  ```

In `crates/hennery-kernel/src/capabilities.rs`, replace:

  ```rust
              serde_json::json!({"mode": "full", "features": ["mcp_connections"]})
  ```

with:

  ```rust
              serde_json::json!({"mode": "full", "features": ["mcp_connections", "mcp_stdio"]})
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
          gateway.store.clone(),
          gateway.key.clone(),
  ```

with:

  ```rust
          &gateway,
  ```

- [ ] **Step 4: Run the checks**

   Let cargo add `tokio-util` and `sha2` to the gateway's entry in `Cargo.lock` (no new crate), then run the five checks:

   ```sh
   nix develop -c cargo check --workspace --all-targets
   ```

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "feat(gateway): stdio sets, the sessions' side of minting and revoking, a revoke ends the token's streams"
   ```

### Task 4: The sessions: delivery at every start and resume, revoke at every site, redaction, wiring

**Files:** Create `crates/hennery-sessions/src/redact.rs`, `tests/session_mcp.rs`, `crates/hennery-testkit/tests/session_gateway.rs`, `session_gateway_log.rs`; modify `crates/hennery-sessions/Cargo.toml`, `src/api.rs`, `hosts.rs`, `lib.rs`, `store.rs`, `ws.rs`, `tests/owner.rs`, `tests/store.rs`; `crates/hennery/src/main.rs`; the testkit's `Cargo.toml`, `src/lib.rs`, `src/bin/hennery-fake-acp.rs`, `tests/owner_filter.rs`, `tests/reconcile.rs`.
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/tests/owner.rs`, replace:

  ```rust
              ALTER TABLE sessions DROP COLUMN last_event_id;
              ALTER TABLE session_catalog DROP COLUMN commands;
  ```

with:

  ```rust
              ALTER TABLE sessions DROP COLUMN last_event_id;
              ALTER TABLE sessions DROP COLUMN mcp_delivery_mode;
              ALTER TABLE sessions DROP COLUMN mcp_delivery_servers;
              ALTER TABLE sessions DROP COLUMN mcp_delivery_at;
              ALTER TABLE session_catalog DROP COLUMN commands;
  ```

Create `crates/hennery-sessions/tests/session_mcp.rs`:

  ```rust
  //! Plan 8e: sessions get their MCP servers through `SessionMcp`, inside
  //! their own transitions (lane L1). The delivery decision in each of its
  //! outcomes (lane L2); a failed mint rolls the transition back; every
  //! revoke site of lane L4 revokes in its transaction and cuts the token's
  //! open streams once it commits (the fleet parent's ruling), and a
  //! presumed park or a stale fact does not; a token the agent printed is
  //! stored and published redacted (decision 11).

  use hennery_gateway::api::GatewayState;
  use hennery_gateway::key::MasterKey;
  use hennery_gateway::model::{Change, CredKind, NewConnection};
  use hennery_gateway::revocation::{Revocations, Watch};
  use hennery_gateway::scope::{ClientIdentity, ProxyStore};
  use hennery_gateway::session::GatewayMcp;
  use hennery_gateway::stdio::StdioInput;
  use hennery_gateway::store::GatewayStore;
  use hennery_kernel::hats::HatChange;
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::{Operator, SetupOutcome};
  use hennery_kernel::secret::{sha256_hex, unix_now};
  use hennery_proto::frames::{McpIsolation, McpServer, ParkReason, SessionBody};
  use hennery_proto::rest::McpSessionDeliveryMode;
  use hennery_sessions::store::{Deletion, McpContext, McpGiven, Reassign, ResumeRequest, Store, Unattached};
  use serde_json::json;
  use std::path::PathBuf;
  use std::sync::Arc;

  const ORIGIN: &str = "https://hennery.example";
  const HOST: &str = "h1";

  const CLAUDE: McpContext = McpContext {
      capable: true,
      isolation: McpIsolation::ClaudeStrict,
      rules_name_other_hats: false,
  };
  const CODEX: McpContext = McpContext {
      capable: true,
      isolation: McpIsolation::None,
      rules_name_other_hats: false,
  };

  struct World {
      _dir: tempfile::TempDir,
      db: PathBuf,
      store: Store,
      gateway: GatewayStore,
      /// The gateway as the store was given it, for another store.
      state: GatewayState,
      proxy: ProxyStore,
      revocations: Revocations,
      key: MasterKey,
      /// The host's default hat, with `linear` mounted on it.
      hat: String,
      /// Another hat, with `acme` mounted on the host.
      work: String,
  }

  fn connection(gateway: &GatewayStore, slug: &str, hat: &str) {
      let new = NewConnection {
          slug: slug.into(),
          label: slug.into(),
          url: "http://127.0.0.1:9/mcp".into(),
          hat_id: hat.into(),
          cred_kind: CredKind::None,
          static_header: None,
          static_prefix: None,
          tool_allowlist: None,
          internal_network: true,
      };
      let Change::Done(record) = gateway.create(&new, unix_now()).unwrap() else {
          panic!("no connection");
      };
      assert!(matches!(
          gateway.replace_mounts(&record.id, &[HOST.to_string()]).unwrap(),
          Change::Done(_)
      ));
  }

  impl World {
      fn new() -> Self {
          Self::with_setup(true)
      }

      /// `set_up`: the owner is set up at `ORIGIN`, so the gateway has a
      /// `public_url`.
      fn with_setup(set_up: bool) -> Self {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let hosts = Hosts::open(&db).unwrap();
          let enrollment = Enrollment {
              public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
              name: "laptop".into(),
              host_version: "0".into(),
              platform: "linux".into(),
          };
          hosts.register(HOST, &enrollment, 1).unwrap();
          let hat = hosts.host(HOST).unwrap().unwrap().default_hat_id;
          let HatChange::Done(work) = hosts.create_hat("Work", None, 1).unwrap() else {
              panic!("no hat");
          };
          let operator = Operator::open(&db).unwrap();
          if set_up {
              let now = unix_now();
              let token = operator.issue_setup_token(now).unwrap().unwrap();
              let SetupOutcome::Done { .. } = operator.set_up(&token, "correct horse battery", ORIGIN, now).unwrap()
              else {
                  panic!("setup failed");
              };
          }
          let gateway_store = Arc::new(GatewayStore::open(&db).unwrap());
          connection(&gateway_store, "linear", &hat);
          connection(&gateway_store, "acme", &work.id);
          let revocations = Revocations::new();
          let key = MasterKey::from_bytes([7; 32]);
          let gateway = GatewayState {
              store: gateway_store,
              key: Arc::new(MasterKey::from_bytes([7; 32])),
              operator: Arc::new(operator),
              revocations: revocations.clone(),
          };
          let store = Store::open(&db).unwrap();
          store.set_session_mcp(Arc::new(GatewayMcp::new(&gateway)));
          Self {
              proxy: ProxyStore::open(&db).unwrap(),
              gateway: GatewayStore::open(&db).unwrap(),
              state: gateway,
              _dir: dir,
              db,
              store,
              revocations,
              key,
              hat,
              work: work.id,
          }
      }

      fn start(&self, id: &str, hat: &str, mcp: McpContext) -> McpGiven {
          self.store
              .create_session_with_mcp(id, HOST, "agent", "/p", hat, None, mcp)
              .unwrap()
              .expect("created")
      }

      /// Started and `active`, with its token.
      fn active(&self, id: &str) -> String {
          let given = self.start(id, &self.hat.clone(), CLAUDE);
          self.store
              .ingest(id, 1, &SessionBody::session_started("r0", format!("agent-{id}")))
              .unwrap();
          token(&given)
      }

      fn live(&self, token: &str) -> bool {
          self.proxy.resolve(token, unix_now()).unwrap().is_some()
      }

      fn revoked_row(&self, token: &str) -> bool {
          rusqlite::Connection::open(&self.db)
              .unwrap()
              .query_row(
                  "SELECT revoked_at IS NOT NULL FROM gw_session_tokens WHERE token_hash = ?1",
                  [sha256_hex(token.as_bytes())],
                  |r| r.get(0),
              )
              .unwrap()
      }

      fn token_rows(&self, session: &str) -> i64 {
          rusqlite::Connection::open(&self.db)
              .unwrap()
              .query_row(
                  "SELECT count(*) FROM gw_session_tokens WHERE session_id = ?1",
                  [session],
                  |r| r.get(0),
              )
              .unwrap()
      }

      fn watch(&self, token: &str) -> Watch {
          self.revocations.watch(&sha256_hex(token.as_bytes()))
      }

      /// The token is revoked in its row and its watch cut: what every revoke
      /// site must do.
      fn assert_revoked(&self, token: &str, watch: &Watch, site: &str) {
          assert!(self.revoked_row(token), "{site}: the token's row is still live");
          assert!(!self.live(token), "{site}: the token still resolves");
          assert!(watch.token().is_cancelled(), "{site}: its open streams were not cut");
      }

      fn assert_live(&self, token: &str, watch: &Watch, site: &str) {
          assert!(self.live(token), "{site}: the token was revoked");
          assert!(!watch.token().is_cancelled(), "{site}: its streams were cut");
      }

      fn lifecycle(&self, id: &str) -> String {
          self.store.find_session(id).unwrap().unwrap().lifecycle
      }
  }

  fn token(given: &McpGiven) -> String {
      let Some(McpServer::Http { headers, .. }) = given.servers.first() else {
          panic!("no http server: {given:?}");
      };
      headers[0].value.strip_prefix("Bearer ").unwrap().to_string()
  }

  fn names(given: &McpGiven) -> Vec<&str> {
      given
          .servers
          .iter()
          .map(|s| match s {
              McpServer::Http { name, .. } | McpServer::Stdio { name, .. } => name.as_str(),
          })
          .collect()
  }

  // The delivery decision (umbrella §8.5, lane L2), one outcome each.

  #[test]
  fn claude_gets_its_hats_servers_isolated() {
      let w = World::new();
      let given = w.start("s1", &w.hat.clone(), CLAUDE);
      assert_eq!(given.mode, McpSessionDeliveryMode::Isolated);
      assert_eq!(names(&given), ["hennery-linear"]);
      let McpServer::Http { url, .. } = &given.servers[0] else {
          panic!()
      };
      assert_eq!(url, &format!("{ORIGIN}/mcp/linear"));
      let principal = w.proxy.resolve(&token(&given), unix_now()).unwrap().unwrap();
      assert_eq!(principal.hat_id, w.hat);
      assert!(!given.clone().frame().isolation_waived);
      let recorded = w.store.mcp_delivery("s1").unwrap().unwrap();
      assert_eq!((recorded.mode, recorded.servers), (McpSessionDeliveryMode::Isolated, 1));
  }

  #[test]
  fn claude_in_another_hat_gets_that_hats_servers_only() {
      let w = World::new();
      let given = w.start("s1", &w.work.clone(), CLAUDE);
      assert_eq!(given.mode, McpSessionDeliveryMode::Isolated);
      assert_eq!(names(&given), ["hennery-acme"]);
      assert_eq!(
          w.proxy.resolve(&token(&given), unix_now()).unwrap().unwrap().hat_id,
          w.work
      );
  }

  /// The default hat of a host that cannot isolate the agent: its servers,
  /// isolation waived in the frame (plan 8c's hand-off).
  #[test]
  fn codex_in_the_default_hat_is_unisolated() {
      let w = World::new();
      let given = w.start("s1", &w.hat.clone(), CODEX);
      assert_eq!(given.mode, McpSessionDeliveryMode::Unisolated);
      assert_eq!(names(&given), ["hennery-linear"]);
      assert!(given.clone().frame().isolation_waived);
  }

  /// "A mixed host gives Codex nothing" (the brief): a session outside the
  /// default hat counts itself, so its host is mixed, and it falls back.
  #[test]
  fn codex_in_another_hat_falls_back_to_nothing() {
      let w = World::new();
      let given = w.start("s1", &w.work.clone(), CODEX);
      assert_eq!(given.mode, McpSessionDeliveryMode::Fallback);
      assert!(given.servers.is_empty());
      assert_eq!(w.token_rows("s1"), 0, "no token is minted for a fallback");
      let recorded = w.store.mcp_delivery("s1").unwrap().unwrap();
      assert_eq!((recorded.mode, recorded.servers), (McpSessionDeliveryMode::Fallback, 0));
      assert!(!given.frame().isolation_waived);
  }

  /// Mixedness never moves the default hat: a Codex session there still gets
  /// its servers while another hat's session is live, or a rule names one.
  #[test]
  fn a_mixed_host_keeps_the_default_hats_servers() {
      let w = World::new();
      w.start("other", &w.work.clone(), CLAUDE);
      let given = w.start("s1", &w.hat.clone(), CODEX);
      assert_eq!(given.mode, McpSessionDeliveryMode::Unisolated);
      let ruled = McpContext {
          rules_name_other_hats: true,
          ..CODEX
      };
      assert_eq!(
          w.start("s2", &w.hat.clone(), ruled).mode,
          McpSessionDeliveryMode::Unisolated
      );
  }

  /// Lane L3: a host without `mcp_servers` gets no server and no token.
  #[test]
  fn a_host_without_the_capability_gets_nothing() {
      let w = World::new();
      let given = w.start("s1", &w.hat.clone(), McpContext::NONE);
      assert_eq!(given.mode, McpSessionDeliveryMode::Unsupported);
      assert!(given.servers.is_empty());
      assert_eq!(w.token_rows("s1"), 0);
      let recorded = w.store.mcp_delivery("s1").unwrap().unwrap();
      assert_eq!(recorded.mode, McpSessionDeliveryMode::Unsupported);
  }

  #[test]
  fn a_session_not_started_since_plan_8e_has_no_record() {
      let w = World::new();
      rusqlite::Connection::open(&w.db)
          .unwrap()
          .execute(
              "INSERT INTO sessions(id, host_id, agent, cwd, hat_id, lifecycle, created_at, last_event_at, owner_id)
               SELECT 'old', ?1, 'agent', '/p', ?2, 'parked', 't', 't', owner_id FROM hosts WHERE id = ?1",
              [HOST, &w.hat],
          )
          .unwrap();
      assert_eq!(w.store.mcp_delivery("old").unwrap(), None);
  }

  // Lane L1: a failed mint rolls the transition back.

  /// A stdio server whose values no longer open (another key's, or moved):
  /// the delivery read fails.
  fn break_stdio(w: &World) {
      let files = StdioInput {
          name: "files".into(),
          command: "files-mcp".into(),
          args: vec![],
          env: vec![("KEY".into(), Some("v".into()))],
      };
      let _ = w
          .gateway
          .replace_stdio_set(HOST, &w.hat, &[files], &w.key, unix_now())
          .unwrap();
      rusqlite::Connection::open(&w.db)
          .unwrap()
          .execute("UPDATE gw_stdio_servers SET env_ciphertext = zeroblob(64)", [])
          .unwrap();
  }

  #[test]
  fn a_start_whose_mint_fails_stores_no_session() {
      let w = World::with_setup(false);
      let failed = w
          .store
          .create_session_with_mcp("s1", HOST, "agent", "/p", &w.hat.clone(), None, CLAUDE);
      assert!(failed.is_err());
      assert_eq!(w.store.find_session("s1").unwrap(), None);
      assert_eq!(w.token_rows("s1"), 0);
      let w = World::new();
      break_stdio(&w);
      assert!(
          w.store
              .create_session_with_mcp("s2", HOST, "agent", "/p", &w.hat.clone(), None, CLAUDE)
              .is_err()
      );
      assert_eq!(w.store.find_session("s2").unwrap(), None);
  }

  #[test]
  fn a_resume_whose_mint_fails_leaves_the_session_parked() {
      let w = World::new();
      let first = w.active("s1");
      w.store
          .ingest(
              "s1",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      break_stdio(&w);
      assert!(w.store.request_resume_with_mcp("s1", &w.hat.clone(), CLAUDE).is_err());
      assert_eq!(w.lifecycle("s1"), "parked");
      assert!(w.revoked_row(&first), "the old token stays revoked");
  }

  /// ACP core §4.3: every resume mints a fresh token that supersedes the
  /// one before; the old one's streams are cut once the resume commits.
  #[test]
  fn a_resume_supersedes_the_token_and_cuts_the_old_ones_streams() {
      let w = World::new();
      let first = w.active("s1");
      w.store
          .ingest(
              "s1",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      let watch = w.watch(&first);
      let ResumeRequest::Starting { mcp, .. } = w.store.request_resume_with_mcp("s1", &w.hat.clone(), CLAUDE).unwrap()
      else {
          panic!("not resumed");
      };
      let second = token(&mcp);
      assert_ne!(first, second);
      assert!(watch.token().is_cancelled());
      assert!(!w.live(&first));
      assert!(w.live(&second));
  }

  // Lane L4: every revoke site, each on a session whose token is live there.

  #[test]
  fn a_host_reported_park_revokes() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      w.store
          .ingest(
              "s1",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      w.assert_revoked(&token, &watch, "session_parked");
  }

  #[test]
  fn a_host_reported_close_revokes() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      w.store.ingest("s1", 2, &SessionBody::SessionClosed).unwrap();
      w.assert_revoked(&token, &watch, "session_closed");
  }

  #[test]
  fn an_adapter_exit_revokes_ahead_of_its_park() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      let exited = SessionBody::AdapterExited {
          code: Some(1),
          signal: None,
          stderr_tail: String::new(),
      };
      w.store.ingest("s1", 2, &exited).unwrap();
      assert_eq!(w.lifecycle("s1"), "active", "the park comes after");
      w.assert_revoked(&token, &watch, "adapter_exited");
  }

  #[test]
  fn a_failed_start_revokes() {
      let w = World::new();
      let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
      let watch = w.watch(&token_of);
      let failed = SessionBody::StartFailed {
          request_id: "r0".into(),
          code: "start_failed".into(),
          message: "no".into(),
      };
      w.store.ingest("s1", 1, &failed).unwrap();
      w.assert_revoked(&token_of, &watch, "start_failed");
  }

  #[test]
  fn a_start_the_route_fails_revokes() {
      let w = World::new();
      let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
      let watch = w.watch(&token_of);
      w.store.mark_failed("s1", "mcp_isolation_unavailable").unwrap();
      w.assert_revoked(&token_of, &watch, "mark_failed");
  }

  #[test]
  fn a_resume_the_route_fails_revokes() {
      let w = World::new();
      let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
      let watch = w.watch(&token_of);
      w.store
          .mark_failed_if_starting("s1", "mcp_isolation_unavailable")
          .unwrap();
      w.assert_revoked(&token_of, &watch, "mark_failed_if_starting");
  }

  /// `close_now`: an active session on a host that is away.
  #[test]
  fn closing_an_unattached_session_revokes() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      w.store.close_now("s1").unwrap();
      w.assert_revoked(&token, &watch, "close_now");
  }

  #[test]
  fn a_rejected_reconcile_close_revokes() {
      let w = World::new();
      let token = w.active("s1");
      w.store.record_close_request("s1").unwrap();
      let watch = w.watch(&token);
      w.store.close_after_rejected_reconcile_close("s1").unwrap();
      assert_eq!(w.lifecycle("s1"), "closed");
      w.assert_revoked(&token, &watch, "close_after_rejected_reconcile_close");
  }

  #[test]
  fn reconcile_revokes_a_start_the_host_never_got() {
      let w = World::new();
      let token_of = token(&w.start("s1", &w.hat.clone(), CLAUDE));
      let watch = w.watch(&token_of);
      w.store.reconcile_host(HOST, &[]).unwrap();
      assert_eq!(w.lifecycle("s1"), "failed");
      w.assert_revoked(&token_of, &watch, "reconcile start_not_delivered");
  }

  #[test]
  fn reconcile_revokes_a_session_a_restarted_host_lost() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      w.store.reconcile_host(HOST, &[]).unwrap();
      assert_eq!(w.lifecycle("s1"), "parked");
      w.assert_revoked(&token, &watch, "reconcile host_restarted");
  }

  /// The row itself, not only the proxy's view: the proxy refuses a revoked
  /// host's tokens by its join anyway, which would hide a missed revoke.
  #[test]
  fn a_host_revoke_revokes_every_token_of_the_host() {
      let w = World::new();
      let one = w.active("s1");
      let two = token(&w.start("s2", &w.hat.clone(), CLAUDE));
      let (watch_one, watch_two) = (w.watch(&one), w.watch(&two));
      w.store.revoke_host(HOST).unwrap();
      w.assert_revoked(&one, &watch_one, "revoke_host (active)");
      w.assert_revoked(&two, &watch_two, "revoke_host (starting)");
  }

  /// A re-assigned session has no running adapter, so no live token should
  /// be left; one a missed revoke (or a database from before 8e) left would
  /// reach the old hat's connections (plan 8d's O8).
  #[test]
  fn a_reassignment_revokes_a_token_left_live() {
      let w = World::new();
      let token = w.active("s1");
      w.store
          .ingest(
              "s1",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      // Left live by hand.
      rusqlite::Connection::open(&w.db)
          .unwrap()
          .execute("UPDATE gw_session_tokens SET revoked_at = NULL", [])
          .unwrap();
      assert!(w.live(&token));
      let watch = w.watch(&token);
      assert!(matches!(
          w.store.reassign_hat("s1", &w.work.clone()).unwrap(),
          Reassign::Done(_)
      ));
      w.assert_revoked(&token, &watch, "reassign_hat");
  }

  /// The purge lane's marker: a delete revokes in its transaction.
  #[test]
  fn a_delete_revokes() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      let unattached = Unattached {
          lifecycle: "active".into(),
          presumed_parked: false,
      };
      assert!(matches!(
          w.store.delete_session("s1", Some(&unattached)).unwrap(),
          Deletion::Done { .. }
      ));
      w.assert_revoked(&token, &watch, "delete_session");
      // The tombstone keeps nothing of what it was given (plan 9a's scrub).
      let kept: (Option<String>, Option<i64>, Option<String>) = rusqlite::Connection::open(&w.db)
          .unwrap()
          .query_row(
              "SELECT mcp_delivery_mode, mcp_delivery_servers, mcp_delivery_at FROM sessions WHERE id = 's1'",
              [],
              |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
          )
          .unwrap();
      assert_eq!(kept, (None, None, None));
  }

  /// The delete's own revoke (the purge lane's marker), beyond `close_in`'s:
  /// a closed session whose token a missed revoke left live.
  #[test]
  fn a_delete_revokes_a_token_left_live_on_a_closed_session() {
      let w = World::new();
      let token = w.active("s1");
      w.store.ingest("s1", 2, &SessionBody::SessionClosed).unwrap();
      rusqlite::Connection::open(&w.db)
          .unwrap()
          .execute("UPDATE gw_session_tokens SET revoked_at = NULL", [])
          .unwrap();
      let watch = w.watch(&token);
      assert!(matches!(
          w.store.delete_session("s1", None).unwrap(),
          Deletion::Done { .. }
      ));
      w.assert_revoked(&token, &watch, "delete_session of a closed session");
  }

  /// The kernel's purge hook (`LifecycleHooks::on_hat_purged`, lane L6):
  /// the sessions' part, then the gateway's, so the hat's row can go.
  #[test]
  fn the_purge_hook_runs_the_gateways_part_too() {
      use hennery_kernel::lifecycle::LifecycleHooks;
      let w = World::new();
      w.active("s1");
      let given = w.start("s2", &w.work.clone(), CLAUDE);
      w.store
          .ingest("s2", 1, &SessionBody::session_started("r0", "agent-s2"))
          .unwrap();
      w.store
          .ingest(
              "s2",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      let hosts = Hosts::open(&w.db).unwrap();
      assert!(matches!(
          hosts.begin_purge(&w.work, unix_now()).unwrap(),
          hennery_kernel::hats::PurgeStart::Frozen { .. }
      ));
      let state = hennery_sessions::AppState::new(Store::open(&w.db).unwrap(), hosts, Operator::open(&w.db).unwrap());
      state.store.set_session_mcp(Arc::new(GatewayMcp::new(&w.state)));
      state.on_hat_purged(&w.work).unwrap();
      assert_eq!(w.token_rows("s2"), 0, "the hat's tokens went");
      assert!(!w.live(&token(&given)));
      let left: i64 = rusqlite::Connection::open(&w.db)
          .unwrap()
          .query_row(
              "SELECT count(*) FROM gw_connections WHERE hat_id = ?1",
              [&w.work],
              |r| r.get(0),
          )
          .unwrap();
      assert_eq!(left, 0, "the hat's connections went");
      assert_eq!(w.token_rows("s1"), 1, "another hat's token stays");
  }

  /// Lane L6: the gateway's part of a hat's purge deletes the hat's tokens
  /// and cuts them; another hat's are left.
  #[test]
  fn a_hat_purge_takes_and_cuts_its_tokens() {
      let w = World::new();
      let theirs = token(&w.start("s1", &w.work.clone(), CLAUDE));
      let mine = token(&w.start("s2", &w.hat.clone(), CLAUDE));
      let (watch_theirs, watch_mine) = (w.watch(&theirs), w.watch(&mine));
      w.store.purge_gateway_hat(&w.work).unwrap();
      assert!(!w.live(&theirs));
      assert_eq!(w.token_rows("s1"), 0);
      assert!(watch_theirs.token().is_cancelled());
      w.assert_live(&mine, &watch_mine, "another hat's purge");
      // Idempotent.
      w.store.purge_gateway_hat(&w.work).unwrap();
  }

  // What does not revoke.

  /// Lane L4's negative: a presumed park is a guess that the host may still
  /// run the session; its token stays.
  #[test]
  fn a_presumed_park_does_not_revoke() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      w.store.presume_parked(HOST).unwrap();
      assert_eq!(w.lifecycle("s1"), "parked");
      w.assert_live(&token, &watch, "presume_parked");
  }

  /// The advisor's race: facts that do not apply to the session as it is now
  /// (a resume in flight) must not end its fresh token.
  #[test]
  fn a_stale_fact_does_not_revoke_a_resumes_fresh_token() {
      let w = World::new();
      w.active("s1");
      w.store
          .ingest(
              "s1",
              2,
              &SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          )
          .unwrap();
      let ResumeRequest::Starting { mcp, .. } = w.store.request_resume_with_mcp("s1", &w.hat.clone(), CLAUDE).unwrap()
      else {
          panic!("not resumed");
      };
      let fresh = token(&mcp);
      let watch = w.watch(&fresh);
      for (seq, stale) in [
          (
              3,
              SessionBody::SessionParked {
                  reason: ParkReason::Idle,
              },
          ),
          (4, SessionBody::SessionClosed),
          (
              5,
              SessionBody::AdapterExited {
                  code: Some(1),
                  signal: None,
                  stderr_tail: String::new(),
              },
          ),
      ] {
          w.store.ingest("s1", seq, &stale).unwrap();
          assert_eq!(w.lifecycle("s1"), "starting", "{stale:?}");
          w.assert_live(&fresh, &watch, &format!("{stale:?}"));
      }
  }

  #[test]
  fn a_start_failed_that_does_not_apply_does_not_revoke() {
      let w = World::new();
      let token = w.active("s1");
      let watch = w.watch(&token);
      let failed = SessionBody::StartFailed {
          request_id: "r0".into(),
          code: "start_failed".into(),
          message: "late".into(),
      };
      w.store.ingest("s1", 2, &failed).unwrap();
      assert_eq!(w.lifecycle("s1"), "active");
      w.assert_live(&token, &watch, "a late start_failed");
  }

  // Decision 11 (the maintainer's open Q2, default chosen): a token the agent
  // printed is stored and published redacted.

  #[test]
  fn a_token_in_an_acp_payload_is_stored_and_published_redacted() {
      let w = World::new();
      let token = w.active("s1");
      let printed = SessionBody::AcpUpdate {
          indexed: Default::default(),
          payload: json!({"sessionUpdate": "agent_message_chunk",
                          "content": {"type": "text", "text": format!("my token is {token}")}}),
      };
      let published = w.store.ingest("s1", 2, &printed).unwrap();
      let stored = w.store.events("s1", 0, 100).unwrap();
      for (what, text) in [
          ("published", serde_json::to_string(&published).unwrap()),
          ("stored", serde_json::to_string(&stored).unwrap()),
      ] {
          assert!(!text.contains(&token), "{what}: {text}");
          assert!(text.contains("my token is hnry_session_<redacted>"), "{what}: {text}");
      }
      // The host re-sending it is the same fact, not a conflict.
      assert!(w.store.ingest("s1", 2, &printed).unwrap().is_empty());
      let raw = std::fs::read(&w.db).unwrap();
      let wal = std::fs::read(w.db.with_extension("db-wal")).unwrap_or_default();
      for bytes in [raw, wal] {
          assert!(
              !bytes.windows(token.len()).any(|window| window == token.as_bytes()),
              "the token is in the database"
          );
      }
  }

  /// Every place a fact's text is extracted from reads the redacted body: a
  /// question's title, the session's title.
  #[test]
  fn a_token_in_a_question_or_a_title_is_redacted_too() {
      let w = World::new();
      let token = w.active("s1");
      let question: SessionBody = serde_json::from_value(json!({
          "kind": "pending_opened", "pending_id": "p1",
          "indexed": {"pending": {"id": "p1", "kind": "permission", "option_ids": ["allow"],
                                  "title": format!("run {token}?")}},
          "payload": {"toolCall": {"title": format!("run {token}?")}}
      }))
      .unwrap();
      w.store.ingest("s1", 2, &question).unwrap();
      let pending = serde_json::to_string(&w.store.open_pending("s1").unwrap()).unwrap();
      assert!(!pending.contains(&token), "{pending}");
      let titled = SessionBody::AcpUpdate {
          indexed: serde_json::from_value(json!({"title": format!("about {token}")})).unwrap(),
          payload: json!({"sessionUpdate": "session_info_update", "title": format!("about {token}")}),
      };
      w.store.ingest("s1", 3, &titled).unwrap();
      let item = serde_json::to_string(&w.store.find_session_item("s1").unwrap()).unwrap();
      assert!(!item.contains(&token), "{item}");
  }

  /// The advisor's 7: redaction reads a body back from its JSON, so that
  /// round trip must be the identity for every kind of body.
  #[test]
  fn every_body_reads_back_as_it_was_written() {
      let bodies = [
          json!({"kind": "session_started", "request_id": "r", "agent_session_id": "a",
                 "indexed": {"title": "t", "current_model": "m"}}),
          json!({"kind": "start_failed", "request_id": "r", "code": "c", "message": "m"}),
          json!({"kind": "turn_started", "request_id": "r", "turn_id": "t"}),
          json!({"kind": "acp_update", "indexed": {"turn_id": "t", "early": true}, "payload": {"a": [1, "b"]}}),
          json!({"kind": "turn_ended", "turn_id": "t", "outcome": "failed", "stop_reason": "s", "error": "e"}),
          json!({"kind": "session_parked", "reason": "adapter_exited"}),
          json!({"kind": "session_closed"}),
          json!({"kind": "adapter_exited", "code": 1, "signal": 9, "stderr_tail": "x"}),
          json!({"kind": "host_note", "note": "n", "text": "t"}),
          json!({"kind": "config_applied", "request_id": "r", "indexed": {"current_mode": "m"}}),
          json!({"kind": "pending_opened", "pending_id": "p",
                 "indexed": {"pending": {"id": "p", "kind": "elicitation", "title": "t"}}, "payload": {}}),
          json!({"kind": "pending_resolved", "pending_id": "p", "resolution": "cancelled", "reason": "session_closed"}),
          json!({"kind": "answer_result", "pending_id": "p", "request_id": "r", "delivered": true}),
          json!({"kind": "git_state", "branch": "b", "dirty": true, "worktree": false, "head": "h", "base_commit": "c"}),
      ];
      let mut kinds = std::collections::BTreeSet::new();
      for json in bodies {
          let body: SessionBody = serde_json::from_value(json.clone()).unwrap_or_else(|e| panic!("{json}: {e}"));
          kinds.insert(json["kind"].as_str().unwrap().to_string());
          let again: SessionBody = serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
          assert_eq!(again, body, "{json}");
      }
      assert_eq!(kinds.len(), 14, "one of each kind");
  }
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
               ALTER TABLE sessions DROP COLUMN last_event_id;
               DROP TABLE event_attachments;
  ```

with:

  ```rust
               ALTER TABLE sessions DROP COLUMN last_event_id;
               ALTER TABLE sessions DROP COLUMN mcp_delivery_mode;
               ALTER TABLE sessions DROP COLUMN mcp_delivery_servers;
               ALTER TABLE sessions DROP COLUMN mcp_delivery_at;
               DROP TABLE event_attachments;
  ```

In `crates/hennery-sessions/tests/store.rs`, replace:

  ```rust
           ALTER TABLE sessions DROP COLUMN hat_rule_id;
           PRAGMA user_version = 9;",
  ```

with:

  ```rust
           ALTER TABLE sessions DROP COLUMN hat_rule_id;
           ALTER TABLE sessions DROP COLUMN mcp_delivery_mode;
           ALTER TABLE sessions DROP COLUMN mcp_delivery_servers;
           ALTER TABLE sessions DROP COLUMN mcp_delivery_at;
           PRAGMA user_version = 9;",
  ```

In `crates/hennery-testkit/Cargo.toml`, replace:

  ```toml
  anyhow.workspace = true
  futures.workspace = true
  ```

with:

  ```toml
  anyhow.workspace = true
  # A fake MCP upstream for the gateway end to end (plan 8e).
  axum.workspace = true
  futures.workspace = true
  ```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
      let forms = Arc::new(AtomicBool::new(false));
      Agent
  ```

with:

  ```rust
      let forms = Arc::new(AtomicBool::new(false));
      // The latest `session/new` or `session/load`'s `mcpServers`, as JSON, for
      // `echo_servers`.
      let servers: Arc<Mutex<String>> = Arc::default();
      Agent
  ```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
                  async move |req: NewSessionRequest, responder, cx| {
                      log_session(&script, "session/new", &req);
  ```

with:

  ```rust
                  let servers = servers.clone();
                  async move |req: NewSessionRequest, responder, cx| {
                      log_session(&script, "session/new", &req);
                      *servers.lock().unwrap() = serde_json::to_string(&req.mcp_servers).unwrap();
  ```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
                  async move |req: LoadSessionRequest, responder, cx| {
                      log_session(&script, "session/load", &req);
  ```

with:

  ```rust
                  let servers = servers.clone();
                  async move |req: LoadSessionRequest, responder, cx| {
                      log_session(&script, "session/load", &req);
                      *servers.lock().unwrap() = serde_json::to_string(&req.mcp_servers).unwrap();
  ```

In `crates/hennery-testkit/src/bin/hennery-fake-acp.rs`, replace:

  ```rust
                  async move |req: PromptRequest, responder, cx| {
  ```

with:

  ```rust
                  let servers = servers.clone();
                  async move |req: PromptRequest, responder, cx| {
                      if script.echo_servers {
                          let printed = format!("my servers: {}", servers.lock().unwrap());
                          cx.send_notification(chunk(&req.session_id, printed))?;
                      }
  ```

In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
      pub codex_archive_links_archived: Option<String>,
  }
  ```

with:

  ```rust
      pub codex_archive_links_archived: Option<String>,
      /// At the start of every prompt, print the `mcpServers` of the latest
      /// `session/new` or `session/load` as an agent message chunk, their
      /// headers' values included: an agent that prints its gateway token
      /// (plan 8e decision 11).
      #[serde(default, skip_serializing_if = "std::ops::Not::not")]
      pub echo_servers: bool,
  }
  ```

In `crates/hennery-testkit/src/lib.rs`, replace:

  ```rust
              codex_archive_links_archived: None,
          }
  ```

with:

  ```rust
              codex_archive_links_archived: None,
              echo_servers: false,
          }
  ```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

  ```rust
          128,
  ```

with:

  ```rust
          134,
  ```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

  ```rust
              "open_turn": { "turn_id": turn, "state": "started" }, "pending": []
  ```

with:

  ```rust
              "open_turn": { "turn_id": turn, "state": "started" }, "pending": [],
              // Plan 8e: this scripted host announces no `mcp_servers`.
              "mcp_delivery": { "mode": "unsupported", "servers": 0, "at": item.created_at }
  ```

Create `crates/hennery-testkit/tests/session_gateway.rs`:

  ```rust
  //! Plan 8e end to end: a real collector with the gateway wired in as the
  //! binary wires it (sessions mint and revoke through `GatewayMcp`; the
  //! proxy watches the same `Revocations`), a real host, the fake ACP adapter
  //! as a real child process, and a fake MCP upstream, over real sockets.
  //!
  //! A Claude session on a host with a mounted connection gets exactly its
  //! hat's server with a working token; the token reaches only its hat's
  //! connections; a park ends it, and the stream open on it; a resume brings
  //! a new token, the old one staying dead. Codex on a mixed host gets
  //! nothing outside the default hat, and the default hat's servers inside
  //! it. A token the agent prints never reaches the timeline, its stream or
  //! the log (decision 11, ACP core §8), nor does the upstream's URL past its
  //! origin (lane L11). The whole test runs under the subscriber the process
  //! installs (`logging::capped`) at `trace`, the thread's own: the runtime
  //! is current-thread, so the collector's and the host's tasks log here.

  use axum::body::{Body, Bytes};
  use axum::http::{StatusCode, header};
  use axum::response::Response;
  use futures::StreamExt;
  use hennery_gateway::api::GatewayState;
  use hennery_gateway::key::KeySource;
  use hennery_gateway::model::{Change, CredKind, NewConnection};
  use hennery_gateway::proxy::{Limits, ProxyState};
  use hennery_gateway::scope::ProxyStore;
  use hennery_gateway::session::GatewayMcp;
  use hennery_host::identity::HostKey;
  use hennery_host::profile::Profile;
  use hennery_host::{AgentCommand, HostConfig};
  use hennery_kernel::egress::{Egress, Timeouts};
  use hennery_kernel::hats::{HatChange, NewRule};
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_sessions::{AppState, store::Store};
  use hennery_testkit::{FakeScript, SCRIPT_ENV};
  use serde_json::{Value, json};
  use std::net::SocketAddr;
  use std::path::{Path, PathBuf};
  use std::sync::{Arc, Mutex};
  use std::time::Duration;
  use tracing_subscriber::util::SubscriberInitExt;

  const HOST: &str = "host-1";
  /// A secret in the upstream URL's path: never logged (lane L11).
  const URL_CANARY: &str = "url-c4n4ry-0123456789";
  /// How long anything may take before the test fails.
  const BOUND: Duration = Duration::from_secs(30);

  fn host_key() -> HostKey {
      HostKey::from_seed([1; 32])
  }

  /// A `tracing` writer into a shared buffer.
  #[derive(Clone, Default)]
  struct Captured(Arc<Mutex<Vec<u8>>>);

  impl std::io::Write for Captured {
      fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
          self.0.lock().unwrap().extend_from_slice(buf);
          Ok(buf.len())
      }

      fn flush(&mut self) -> std::io::Result<()> {
          Ok(())
      }
  }

  impl Captured {
      fn text(&self) -> String {
          String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
      }
  }

  /// The fake MCP upstream: a JSON answer to every `POST`, and an event
  /// stream that sends one event and then holds to every `GET`.
  async fn upstream() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
      let seen: Arc<Mutex<Vec<String>>> = Arc::default();
      let app = {
          let seen = seen.clone();
          axum::Router::new().fallback(move |req: axum::extract::Request| {
              let seen = seen.clone();
              async move {
                  let auth = req
                      .headers()
                      .get(header::AUTHORIZATION)
                      .map(|v| v.to_str().unwrap().to_string())
                      .unwrap_or_default();
                  seen.lock()
                      .unwrap()
                      .push(format!("{} {} auth={auth}", req.method(), req.uri()));
                  if req.method() == axum::http::Method::GET {
                      let stream = futures::stream::once(async {
                          Ok::<_, std::io::Error>(Bytes::from("data: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n\n"))
                      })
                      .chain(futures::stream::pending());
                      return Response::builder()
                          .header(header::CONTENT_TYPE, "text/event-stream")
                          .body(Body::from_stream(stream))
                          .unwrap();
                  }
                  Response::builder()
                      .header(header::CONTENT_TYPE, "application/json")
                      .body(Body::from(
                          json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}}).to_string(),
                      ))
                      .unwrap()
              }
          })
      };
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
      (addr, seen)
  }

  struct Collector {
      addr: SocketAddr,
      state: AppState,
      gateway: GatewayState,
      db: PathBuf,
      client: reqwest::Client,
      proxy_client: reqwest::Client,
      hat: String,
      work: String,
  }

  impl Collector {
      async fn start(dir: &Path, upstream: SocketAddr) -> Self {
          let db = dir.join("hennery.db");
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let hosts = Hosts::open(&db).unwrap();
          let enrollment = Enrollment {
              public_key: host_key().public_key_hex(),
              name: "test".into(),
              host_version: "test".into(),
              platform: "test".into(),
          };
          hosts.register(HOST, &enrollment, 0).unwrap();
          let hat = hosts.host(HOST).unwrap().unwrap().default_hat_id;
          let HatChange::Done(work) = hosts.create_hat("Work", None, 0).unwrap() else {
              panic!("no hat");
          };
          let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
          // Set up, so the gateway has a `public_url`.
          let client = hennery_testkit::operator_client(&state.operator);
          // As the binary wires it (`hennery`'s `gateway`).
          let keys = KeySource::from_vars(dir, Some("07".repeat(32).into()), None).unwrap();
          let gateway = hennery_gateway::open(&db, &keys, state.operator.clone()).unwrap();
          state.store.set_session_mcp(Arc::new(GatewayMcp::new(&gateway)));
          let egress = Egress::new(Timeouts {
              connect: Duration::from_secs(5),
              request: Duration::from_secs(60),
          })
          .unwrap();
          let proxy = ProxyState::full(
              Arc::new(ProxyStore::open(&db).unwrap()),
              &gateway,
              egress,
              Limits::default(),
          );
          let router = hennery_sessions::router(state.clone())
              .merge(hennery_gateway::api::router(gateway.clone()))
              .merge(hennery_gateway::proxy::router(proxy));
          tokio::spawn(hennery_sessions::serve_all(
              vec![listener],
              router,
              state.shutdown.clone(),
          ));
          let collector = Self {
              addr,
              state,
              gateway,
              db,
              client,
              proxy_client: reqwest::Client::builder().no_proxy().build().unwrap(),
              hat,
              work: work.id,
          };
          let url = format!("http://{upstream}/mcp/{URL_CANARY}");
          collector.connection("linear", &url, &collector.hat.clone());
          collector.connection("acme", &url, &collector.work.clone());
          collector
      }

      fn connection(&self, slug: &str, url: &str, hat: &str) {
          let new = NewConnection {
              slug: slug.into(),
              label: slug.into(),
              url: url.into(),
              hat_id: hat.into(),
              cred_kind: CredKind::None,
              static_header: None,
              static_prefix: None,
              tool_allowlist: None,
              // Loopback (lane L7): no test-only bypass.
              internal_network: true,
          };
          let Change::Done(record) = self.gateway.store.create(&new, 0).unwrap() else {
              panic!("no connection");
          };
          assert!(matches!(
              self.gateway
                  .store
                  .replace_mounts(&record.id, &[HOST.to_string()])
                  .unwrap(),
              Change::Done(_)
          ));
      }

      fn url(&self, path: &str) -> String {
          format!("http://{}{path}", self.addr)
      }

      async fn json(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
          let mut req = self.client.request(method, self.url(path));
          if let Some(body) = body {
              req = req.json(&body);
          }
          let resp = req.send().await.unwrap();
          let status = resp.status().as_u16();
          (status, resp.json().await.unwrap_or(Value::Null))
      }

      async fn start_session(&self, agent: &str, cwd: &Path) -> String {
          let (status, body) = self
              .json(
                  reqwest::Method::POST,
                  "/api/sessions",
                  Some(json!({"host_id": HOST, "agent": agent, "cwd": cwd})),
              )
              .await;
          assert_eq!(status, 202, "{body}");
          let id = body["session_id"].as_str().unwrap().to_string();
          self.wait_lifecycle(&id, "active").await;
          id
      }

      async fn detail(&self, id: &str) -> Value {
          self.json(reqwest::Method::GET, &format!("/api/sessions/{id}"), None)
              .await
              .1
      }

      async fn wait_lifecycle(&self, id: &str, lifecycle: &str) {
          wait_for(&format!("{id} {lifecycle}"), || async {
              (self.detail(id).await["lifecycle"] == lifecycle).then_some(())
          })
          .await;
      }

      /// `POST /mcp/<slug>` with `token`: the status.
      async fn call(&self, slug: &str, token: &str) -> u16 {
          self.proxy_client
              .post(self.url(&format!("/mcp/{slug}")))
              .bearer_auth(token)
              .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
              .send()
              .await
              .unwrap()
              .status()
              .as_u16()
      }

      fn token_rows(&self, session: &str) -> i64 {
          rusqlite::Connection::open(&self.db)
              .unwrap()
              .query_row(
                  "SELECT count(*) FROM gw_session_tokens WHERE session_id = ?1",
                  [session],
                  |r| r.get(0),
              )
              .unwrap()
      }
  }

  async fn wait_for<T, F, Fut>(what: &str, mut probe: F) -> T
  where
      F: FnMut() -> Fut,
      Fut: std::future::Future<Output = Option<T>>,
  {
      let deadline = tokio::time::Instant::now() + BOUND;
      loop {
          if let Some(v) = probe().await {
              return v;
          }
          assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
          tokio::time::sleep(Duration::from_millis(50)).await;
      }
  }

  /// The host, its `claude` (the pinned adapter's profile: strict, isolated)
  /// and `codex` (no isolation) both the fake adapter logging to `log`.
  fn start_host(collector: SocketAddr, data_dir: &Path, log: &Path) -> tokio::task::JoinHandle<()> {
      let script = FakeScript {
          session_log: Some(log.to_string_lossy().into_owned()),
          echo_servers: true,
          ..Default::default()
      };
      let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
      fake.env
          .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
      let mut cfg = HostConfig::new(
          format!("ws://{collector}/api/hosts/ws"),
          HOST,
          host_key(),
          data_dir.to_path_buf(),
      );
      cfg.reconnect_min = Duration::from_millis(100);
      cfg.reconnect_max = Duration::from_millis(500);
      cfg.agents.insert("claude".into(), fake.clone());
      cfg.agents.insert("codex".into(), fake);
      cfg.profiles.insert("claude".into(), Profile::Claude);
      tokio::spawn(async move {
          hennery_host::run(cfg).await.unwrap();
      })
  }

  /// The `mcpServers` of the adapter's latest `session/new` or
  /// `session/load`.
  fn last_servers(log: &Path) -> Value {
      let text = std::fs::read_to_string(log).unwrap_or_default();
      let line = text.lines().last().expect("no session/new or session/load logged");
      serde_json::from_str::<Value>(line).unwrap()["params"]["mcpServers"].clone()
  }

  fn bearer(servers: &Value) -> String {
      servers[0]["headers"][0]["value"]
          .as_str()
          .unwrap()
          .strip_prefix("Bearer ")
          .unwrap()
          .to_string()
  }

  async fn host_connected(c: &Collector) {
      wait_for("host connection", || async {
          let (_, hosts) = c.json(reqwest::Method::GET, "/api/hosts", None).await;
          hosts
              .as_array()?
              .iter()
              .any(|h| h["host_id"] == HOST && h["connected"] == true)
              .then_some(())
      })
      .await;
  }

  #[test]
  fn a_claude_session_gets_its_hats_servers_and_a_token_that_ends_with_its_park() {
      let logs = Captured::default();
      let subscriber = hennery_host::logging::capped(
          tracing_subscriber::fmt()
              .with_max_level(tracing::Level::TRACE)
              .with_ansi(false)
              .with_writer({
                  let logs = logs.clone();
                  move || logs.clone()
              })
              .finish(),
      );
      let _default = subscriber.set_default();
      let runtime = tokio::runtime::Builder::new_current_thread()
          .enable_all()
          .build()
          .unwrap();
      let mut seen_tokens = Vec::new();
      runtime.block_on(async {
          let dir = tempfile::tempdir().unwrap();
          let (upstream, upstream_seen) = upstream().await;
          let c = Collector::start(dir.path(), upstream).await;
          let log = dir.path().join("sessions.jsonl");
          let _host = start_host(c.addr, &dir.path().join("host"), &log);
          host_connected(&c).await;

          // The host list shows the isolation its `hello` announced.
          let (_, hosts) = c.json(reqwest::Method::GET, "/api/hosts", None).await;
          assert_eq!(
              hosts[0]["mcp_delivery"],
              json!({"claude": "isolated", "codex": "default_hat_only"}),
              "{hosts}"
          );

          let cwd = std::env::temp_dir();
          let id = c.start_session("claude", &cwd).await;
          // The stdio routes are answered as JSON through the merged router,
          // not by the web UI's catch-all (plan 4b).
          let (status, set) = c
              .json(
                  reqwest::Method::GET,
                  &format!("/api/mcp/stdio-servers?host_id={HOST}&hat_id={}", c.hat),
                  None,
              )
              .await;
          assert_eq!((status, &set["servers"]), (200, &json!([])), "{set}");
          // Exactly the hat's server, with a working token.
          let servers = last_servers(&log);
          let first = bearer(&servers);
          seen_tokens.push(first.clone());
          assert_eq!(
              servers,
              json!([{"type": "http", "name": "hennery-linear", "url": "https://hennery.example/mcp/linear",
                      "headers": [{"name": "Authorization", "value": format!("Bearer {first}")}]}])
          );
          assert_eq!(c.call("linear", &first).await, 200);
          // The upstream got the request, and not the session's token.
          let got = upstream_seen.lock().unwrap().clone();
          assert!(got.iter().any(|r| r.starts_with("POST")), "{got:?}");
          assert!(got.iter().all(|r| !r.contains(&first)), "{got:?}");
          // Another hat's connection is not this token's.
          assert_eq!(c.call("acme", &first).await, 404);
          assert_eq!(
              c.detail(&id).await["mcp_delivery"]["mode"],
              "isolated",
              "{}",
              c.detail(&id).await
          );

          // A stream open on the token.
          let stream = c
              .proxy_client
              .get(c.url("/mcp/linear"))
              .bearer_auth(&first)
              .header(header::ACCEPT, "text/event-stream")
              .send()
              .await
              .unwrap();
          assert_eq!(stream.status(), StatusCode::OK);
          let mut stream = stream.bytes_stream();
          let event = tokio::time::timeout(BOUND, stream.next()).await.unwrap();
          assert!(matches!(event, Some(Ok(_))), "{event:?}");

          // Park: the token is dead, and its stream cut.
          let (status, body) = c
              .json(
                  reqwest::Method::POST,
                  &format!("/api/sessions/{id}/park"),
                  Some(json!({})),
              )
              .await;
          assert!(status < 300, "{status} {body}");
          c.wait_lifecycle(&id, "parked").await;
          let next = tokio::time::timeout(BOUND, stream.next())
              .await
              .expect("the stream was not cut");
          assert!(!matches!(next, Some(Ok(_))), "{next:?}");
          assert_eq!(c.call("linear", &first).await, 404);

          // Resume: a new token works, the old one stays dead.
          let (status, body) = c
              .json(
                  reqwest::Method::POST,
                  &format!("/api/sessions/{id}/resume"),
                  Some(json!({})),
              )
              .await;
          assert!(status < 300, "{status} {body}");
          c.wait_lifecycle(&id, "active").await;
          let second = bearer(&last_servers(&log));
          seen_tokens.push(second.clone());
          assert_ne!(first, second);
          assert_eq!(c.call("linear", &second).await, 200);
          assert_eq!(c.call("linear", &first).await, 404);

          // The agent prints its servers, the token included: the timeline and
          // its stream show it redacted.
          let sse = c
              .client
              .get(c.url(&format!("/api/stream/sessions/{id}")))
              .send()
              .await
              .unwrap();
          let mut sse = sse.bytes_stream();
          let (status, body) = c
              .json(
                  reqwest::Method::POST,
                  &format!("/api/sessions/{id}/prompt"),
                  Some(json!({"content": [{"type": "text", "text": "hi"}]})),
              )
              .await;
          assert!(status < 300, "{status} {body}");
          let mut streamed = String::new();
          while !streamed.contains("my servers") {
              let chunk = tokio::time::timeout(BOUND, sse.next()).await.unwrap().unwrap().unwrap();
              streamed.push_str(&String::from_utf8_lossy(&chunk));
          }
          assert!(!streamed.contains(&second), "the token reached the stream: {streamed}");
          assert!(streamed.contains("hnry_session_<redacted>"), "{streamed}");
          let events = wait_for("the printed servers", || async {
              let (_, events) = c
                  .json(reqwest::Method::GET, &format!("/api/sessions/{id}/events"), None)
                  .await;
              let text = events.to_string();
              text.contains("my servers").then_some(text)
          })
          .await;
          assert!(!events.contains(&second), "the token reached the timeline: {events}");

          // Codex on a mixed host (the work hat's sessions make it so): another
          // hat than the default gets nothing; the default hat its servers.
          let work_dir = tempfile::tempdir().unwrap();
          let work_dir = std::fs::canonicalize(work_dir.path()).unwrap();
          let rules = [NewRule {
              prefix: work_dir.to_string_lossy().into_owned(),
              hat_id: c.work.clone(),
              verified: true,
          }];
          c.state.hosts.replace_path_rules(HOST, &rules).unwrap();
          let fallback = c.start_session("codex", &work_dir).await;
          assert_eq!(last_servers(&log), json!([]));
          assert_eq!(c.detail(&fallback).await["mcp_delivery"]["mode"], "fallback");
          assert_eq!(c.token_rows(&fallback), 0);
          let defaulted = c.start_session("codex", &cwd).await;
          let servers = last_servers(&log);
          assert_eq!(servers[0]["name"], "hennery-linear", "{servers}");
          seen_tokens.push(bearer(&servers));
          assert_eq!(c.detail(&defaulted).await["mcp_delivery"]["mode"], "unisolated");
          // Claude in the work hat: that hat's server, and only it (lane L5:
          // the hat is the rules', never the client's).
          let isolated = c.start_session("claude", &work_dir).await;
          let servers = last_servers(&log);
          assert_eq!(servers[0]["name"], "hennery-acme", "{servers}");
          assert_eq!(servers.as_array().unwrap().len(), 1);
          let token = bearer(&servers);
          seen_tokens.push(token.clone());
          assert_eq!(c.call("acme", &token).await, 200);
          assert_eq!(c.call("linear", &token).await, 404);
          assert_eq!(c.detail(&isolated).await["hat_id"], c.work.as_str());
          // Lane L5: a client naming a hat is not heard; the rules decide, so
          // no request mints a token for a hat the host's rules, its default
          // or a re-assignment did not give the session.
          let (status, body) = c
              .json(
                  reqwest::Method::POST,
                  "/api/sessions",
                  Some(json!({"host_id": HOST, "agent": "claude", "cwd": cwd, "hat_id": c.work})),
              )
              .await;
          assert_eq!(status, 202, "{body}");
          let named = body["session_id"].as_str().unwrap().to_string();
          c.wait_lifecycle(&named, "active").await;
          assert_eq!(c.detail(&named).await["hat_id"], c.hat.as_str());
          let servers = last_servers(&log);
          assert_eq!(servers[0]["name"], "hennery-linear", "{servers}");
          seen_tokens.push(bearer(&servers));
          c.state.shutdown.cancel();
      });
      drop(runtime);
      let logged = logs.text();
      assert!(logged.contains("host connected"), "nothing was logged");
      for token in &seen_tokens {
          assert!(!logged.contains(token.as_str()), "a session token reached the log");
      }
      assert!(
          !logged.contains(URL_CANARY),
          "the upstream's URL past its origin reached the log"
      );
  }
  ```

Create `crates/hennery-testkit/tests/session_gateway_log.rs`:

  ```rust
  //! Plan 8e, ACP core §8: the collector's own log never shows a session
  //! token a host's frame quotes. A frame that does not decode is logged by
  //! its error's kind and place (serde's message quotes the offending value),
  //! and a host's refusal by its message with any token redacted (decision
  //! 11). Under the subscriber the process installs (`logging::capped`), at
  //! `trace`, the thread's own: the runtime is current-thread.

  use futures::{SinkExt, StreamExt};
  use hennery_host::identity::HostKey;
  use hennery_kernel::hosts::{Enrollment, Hosts};
  use hennery_kernel::operator::Operator;
  use hennery_proto::frames::HostFrame;
  use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
  use hennery_sessions::AppState;
  use hennery_sessions::store::Store;
  use std::sync::{Arc, Mutex};
  use std::time::Duration;
  use tokio_tungstenite::tungstenite::Message;
  use tracing_subscriber::util::SubscriberInitExt;

  fn host_key() -> HostKey {
      HostKey::from_seed([1; 32])
  }

  /// A `tracing` writer into a shared buffer.
  #[derive(Clone, Default)]
  struct Captured(Arc<Mutex<Vec<u8>>>);

  impl std::io::Write for Captured {
      fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
          self.0.lock().unwrap().extend_from_slice(buf);
          Ok(buf.len())
      }

      fn flush(&mut self) -> std::io::Result<()> {
          Ok(())
      }
  }

  impl Captured {
      fn text(&self) -> String {
          String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
      }
  }

  #[test]
  fn a_token_a_host_frame_quotes_never_reaches_the_collectors_log() {
      let token = format!("{}{}", hennery_gateway::tokens::SESSION_TOKEN_PREFIX, "5a".repeat(32));
      let logs = Captured::default();
      let subscriber = hennery_host::logging::capped(
          tracing_subscriber::fmt()
              .with_max_level(tracing::Level::TRACE)
              .with_ansi(false)
              .with_writer({
                  let logs = logs.clone();
                  move || logs.clone()
              })
              .finish(),
      );
      let _default = subscriber.set_default();
      let runtime = tokio::runtime::Builder::new_current_thread()
          .enable_all()
          .build()
          .unwrap();
      runtime.block_on(async {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let hosts = Hosts::open(&db).unwrap();
          let enrollment = Enrollment {
              public_key: host_key().public_key_hex(),
              name: "test".into(),
              host_version: "test".into(),
              platform: "test".into(),
          };
          hosts.register("host-1", &enrollment, 0).unwrap();
          let state = AppState::new(Store::open(&db).unwrap(), hosts, Operator::open(&db).unwrap());
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let shutdown = state.shutdown.clone();
          let server = tokio::spawn(hennery_sessions::serve(listener, state));
          let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
              .await
              .unwrap();
          let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
          let (mut sink, mut stream) = ws.split();
          let hello = HostFrame::Hello {
              protocol_version: PROTOCOL_VERSION.into(),
              host_version: "0".into(),
              host_id: "host-1".into(),
              proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
              capabilities: Default::default(),
              workspace_roots: vec![],
              attached_sessions: vec![],
              mcp_isolation: Default::default(),
          };
          sink.send(Message::text(serde_json::to_string(&hello).unwrap()))
              .await
              .unwrap();
          assert!(matches!(stream.next().await, Some(Ok(Message::Text(_)))), "hello_ack");
          // A frame that does not decode, the token where a number goes:
          // serde's error would quote it.
          let undecodable = serde_json::json!({"type": "session", "session_id": "s1", "seq": token, "body": {}});
          sink.send(Message::text(undecodable.to_string())).await.unwrap();
          // A refusal nobody waits for, quoting the token.
          let refusal = HostFrame::Error {
              request_id: "nobody".into(),
              code: "start_failed".into(),
              message: format!("the agent said {token}"),
          };
          sink.send(Message::text(serde_json::to_string(&refusal).unwrap()))
              .await
              .unwrap();
          // Positive signal: both were logged.
          let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
          while !(logs.text().contains("ignoring unknown or invalid frame")
              && logs.text().contains("host refused a request nobody waits for"))
          {
              assert!(tokio::time::Instant::now() < deadline, "not logged: {}", logs.text());
              tokio::time::sleep(Duration::from_millis(20)).await;
          }
          shutdown.cancel();
          let _ = server.await;
      });
      drop(runtime);
      let logged = logs.text();
      assert!(!logged.contains(&token), "a session token reached the log: {logged}");
      assert!(logged.contains("hnry_session_<redacted>"), "{logged}");
  }
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test -p hennery-sessions --locked --test session_mcp
   ```

   It does not compile: `Store::set_session_mcp`, `create_session_with_mcp`, `McpContext`, `McpGiven` and the sessions crate's dependency on the gateway do not exist.

   ```sh
   git add -A && git commit -m "test(sessions): every session gets its hat's MCP servers, and every revoke site revokes"
   ```

- [ ] **Step 3: Implement**

In `crates/hennery-sessions/Cargo.toml`, replace:

  ```toml
  futures.workspace = true
  hennery-kernel.workspace = true
  ```

with:

  ```toml
  futures.workspace = true
  hennery-gateway.workspace = true
  hennery-kernel.workspace = true
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      Reassign, ResumeRequest, SessionRow, Store, Unattached,
  ```

with:

  ```rust
      McpContext, Reassign, ResumeRequest, SessionRow, Store, Unattached,
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
              error(status, &code, message)
  ```

with:

  ```rust
              // The host's words, which can quote what the agent printed
              // (plan 8e decision 11).
              error(status, &code, crate::redact::shown(&message))
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          RequestError::Rejected { code, message } => error(StatusCode::BAD_GATEWAY, &code, message),
  ```

with:

  ```rust
          RequestError::Rejected { code, message } => {
              error(StatusCode::BAD_GATEWAY, &code, crate::redact::shown(&message))
          }
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      let session_id = uuid::Uuid::now_v7().to_string();
      match state.store.create_session(
  ```

with:

  ```rust
      let mcp_context = match mcp_context(&state, &req.host_id, &req.agent) {
          Ok(context) => context,
          Err(err) => return internal(err),
      };
      let session_id = uuid::Uuid::now_v7().to_string();
      let mcp = match state.store.create_session_with_mcp(
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      ) {
          Ok(true) => {}
          // The hat resolved before its purge froze it (plan 9c decision 10c).
          Ok(false) => return hat_purging(&state, &hat.hat_id),
          Err(err) => return internal(err),
      }
  ```

with:

  ```rust
          mcp_context,
      ) {
          Ok(Some(mcp)) => mcp,
          // The hat resolved before its purge froze it (plan 9c decision 10c).
          Ok(None) => return hat_purging(&state, &hat.hat_id),
          Err(err) => return internal(err),
      };
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          // The hat just stored. No servers yet: minting and the delivery
          // decision are plan 8e's.
          hat_id: hat.hat_id.clone(),
          mcp: Default::default(),
  ```

with:

  ```rust
          // The hat just stored, and what it was given (plan 8e).
          hat_id: hat.hat_id.clone(),
          mcp: mcp.frame(),
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          Err(RequestError::NotConnected) => {
              if let Err(e) = state.store.mark_failed(&session_id, "host_offline") {
                  return internal(e);
              }
              request_failed(RequestError::NotConnected)
          }
          // The socket task has already failed the session with the host's
          // code (`Undo::Start`); not for `McpUndeliverable`, which the hub
          // refused before sending: that leaves the session `starting`, and
          // plan 8e fails it (unreachable in 8c, which sends no servers).
          Err(err) => request_failed(err),
      }
  ```

with:

  ```rust
          Err(err @ (RequestError::NotConnected | RequestError::McpUndeliverable)) => {
              start_not_sent(&state, &session_id, err)
          }
          // The socket task has already failed the session with the host's
          // code (`Undo::Start`).
          Err(err) => request_failed(err),
      }
  }

  /// Why a start or resume the hub never sent fails: the host went away, or
  /// its connection no longer takes the servers the decision read (a
  /// reconnect in between; api-8e-8f A2).
  fn not_sent_reason(err: &RequestError) -> &'static str {
      match err {
          RequestError::McpUndeliverable => MCP_UNDELIVERABLE,
          _ => "host_offline",
      }
  }

  /// A start the hub never sent: failed with `not_sent_reason`, its token
  /// revoked (`Store::mark_failed`).
  fn start_not_sent(state: &AppState, session_id: &str, err: RequestError) -> Response {
      if let Err(e) = state.store.mark_failed(session_id, not_sent_reason(&err)) {
          return internal(e);
      }
      request_failed(err)
  }

  /// A resume the hub never sent, as for a start, if it is still `starting`.
  fn resume_not_sent(state: &AppState, session_id: &str, err: RequestError) -> Response {
      if let Err(e) = state.store.mark_failed_if_starting(session_id, not_sent_reason(&err)) {
          return internal(e);
      }
      resume_failed(err)
  }

  /// The code a start or resume the hub refused for its MCP servers fails
  /// with (plan 8c's `mcp_isolation_unavailable`).
  const MCP_UNDELIVERABLE: &str = "mcp_isolation_unavailable";

  /// What the delivery decision reads of `host_id` outside the store (lane
  /// L2): its live connection's capability and `agent`'s isolation, from the
  /// hub, and whether its rules name another hat, from the kernel. A host not
  /// connected takes nothing.
  fn mcp_context(state: &AppState, host_id: &str, agent: &str) -> anyhow::Result<McpContext> {
      let Some((capable, isolation)) = state.hub.mcp_isolation(host_id, agent) else {
          return Ok(McpContext::NONE);
      };
      Ok(McpContext {
          capable,
          isolation,
          rules_name_other_hats: state.hosts.rules_name_other_hats(host_id)?,
      })
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      Json(SessionDetail {
          mcp_delivery: None,
          session: item,
          open_turn,
          pending,
  ```

with:

  ```rust
      let mcp_delivery = match state.store.mcp_delivery(&id) {
          Ok(delivery) => delivery,
          Err(err) => return internal(err),
      };
      Json(SessionDetail {
          session: item,
          open_turn,
          pending,
          mcp_delivery,
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
      let (agent_session_id, committed_seq, config) = match state.store.request_resume(&id, &hat.hat_id) {
  ```

with:

  ```rust
      let mcp_context = match mcp_context(&state, &session.host_id, &session.agent) {
          Ok(context) => context,
          Err(err) => return internal(err),
      };
      let (agent_session_id, committed_seq, config, mcp) = match state.store.request_resume_with_mcp(
          &id,
          &hat.hat_id,
          mcp_context,
      ) {
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
              config,
          }) => {
  ```

with:

  ```rust
              config,
              mcp,
          }) => {
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
              (agent_session_id, committed_seq, config)
  ```

with:

  ```rust
              (agent_session_id, committed_seq, config, mcp)
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          // The hat the resume just re-resolved, equal to the stored one.
          hat_id: hat.hat_id.clone(),
          mcp: Default::default(),
  ```

with:

  ```rust
          // The hat the resume just re-resolved, equal to the stored one, and
          // what it was given, with a fresh token (plan 8e).
          hat_id: hat.hat_id.clone(),
          mcp: mcp.frame(),
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          // Never sent: the host went away since the check above.
          Err(RequestError::NotConnected) => {
              if let Err(e) = state.store.mark_failed_if_starting(&id, "host_offline") {
                  return internal(e);
              }
              resume_failed(RequestError::NotConnected)
          }
          // The socket task has already failed the session with the host's
          // code (`Undo::Start`); not for `McpUndeliverable`, which the hub
          // refused before sending: that leaves the session `starting`, and
          // plan 8e fails it (unreachable in 8c, which sends no servers).
  ```

with:

  ```rust
          // Never sent: the host went away since the check above, or no
          // longer takes the servers.
          Err(err @ (RequestError::NotConnected | RequestError::McpUndeliverable)) => resume_not_sent(&state, &id, err),
          // The socket task has already failed the session with the host's
          // code (`Undo::Start`).
  ```

In `crates/hennery-sessions/src/api.rs`, replace:

  ```rust
          assert_eq!(f.state.store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
      }
  }
  ```

with:

  ```rust
          assert_eq!(f.state.store.find_session("s1").unwrap().unwrap().lifecycle, "starting");
      }
  }

  /// Plan 8e: a start or resume the hub refused to send (`McpUndeliverable`,
  /// a reconnect between the decision and the frame, with no await between
  /// them for a test to step into) fails the session with
  /// `mcp_isolation_unavailable` and answers 409 with it (api-8e-8f A2); one
  /// whose host went away fails `host_offline`, as before.
  #[cfg(test)]
  mod not_sent_tests {
      use super::*;
      use hennery_kernel::hosts::Hosts;
      use hennery_kernel::operator::Operator;

      fn state() -> (tempfile::TempDir, AppState) {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let state = AppState::new(
              Store::open(&db).unwrap(),
              Hosts::open(&db).unwrap(),
              Operator::open(&db).unwrap(),
          );
          (dir, state)
      }

      async fn answer(response: Response) -> (StatusCode, serde_json::Value) {
          let status = response.status();
          let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
          (status, serde_json::from_slice(&body).unwrap())
      }

      fn failure(state: &AppState, id: &str) -> (String, Option<String>) {
          let row = state.store.find_session(id).unwrap().unwrap();
          (row.lifecycle, row.failure_reason)
      }

      #[tokio::test]
      async fn a_start_refused_for_its_servers_fails_mcp_isolation_unavailable() {
          let (_dir, state) = state();
          assert!(state.store.create_session("s1", "h", "a", "/p", "hat", None).unwrap());
          let (status, body) = answer(start_not_sent(&state, "s1", RequestError::McpUndeliverable)).await;
          assert_eq!(
              (status, body["code"].as_str()),
              (StatusCode::CONFLICT, Some(MCP_UNDELIVERABLE))
          );
          assert_eq!(failure(&state, "s1"), ("failed".into(), Some(MCP_UNDELIVERABLE.into())));
      }

      #[tokio::test]
      async fn a_start_never_sent_to_a_gone_host_fails_host_offline() {
          let (_dir, state) = state();
          assert!(state.store.create_session("s1", "h", "a", "/p", "hat", None).unwrap());
          let (status, body) = answer(start_not_sent(&state, "s1", RequestError::NotConnected)).await;
          assert_eq!(
              (status, body["code"].as_str()),
              (StatusCode::CONFLICT, Some("host_offline"))
          );
          assert_eq!(failure(&state, "s1"), ("failed".into(), Some("host_offline".into())));
      }

      #[tokio::test]
      async fn a_resume_refused_for_its_servers_fails_mcp_isolation_unavailable() {
          let (_dir, state) = state();
          assert!(state.store.create_session("s1", "h", "a", "/p", "hat", None).unwrap());
          let (status, body) = answer(resume_not_sent(&state, "s1", RequestError::McpUndeliverable)).await;
          assert_eq!(
              (status, body["code"].as_str()),
              (StatusCode::CONFLICT, Some(MCP_UNDELIVERABLE))
          );
          assert_eq!(failure(&state, "s1"), ("failed".into(), Some(MCP_UNDELIVERABLE.into())));
      }

      #[tokio::test]
      async fn a_resume_never_sent_to_a_gone_host_fails_host_offline() {
          let (_dir, state) = state();
          assert!(state.store.create_session("s1", "h", "a", "/p", "hat", None).unwrap());
          let (status, body) = answer(resume_not_sent(&state, "s1", RequestError::NotConnected)).await;
          assert_eq!(
              (status, body["code"].as_str()),
              (StatusCode::CONFLICT, Some("host_offline"))
          );
          assert_eq!(failure(&state, "s1"), ("failed".into(), Some("host_offline".into())));
      }

      /// The host's words in a refusal are answered with any token redacted
      /// (decision 11).
      #[tokio::test]
      async fn a_refusals_message_is_answered_redacted() {
          let token = format!("{}{}", hennery_gateway::tokens::SESSION_TOKEN_PREFIX, "0a".repeat(32));
          let rejected = || RequestError::Rejected {
              code: "start_failed".into(),
              message: format!("the agent said {token}"),
          };
          for response in [request_failed(rejected()), resume_failed(rejected())] {
              let (_, body) = answer(response).await;
              let message = body["message"].as_str().unwrap();
              assert!(!message.contains(&token) && message.contains("<redacted>"), "{message}");
          }
      }
  }
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
  use hennery_proto::rest::{EnrollRequest, EnrollResponse, HostItem, PairingCodeResponse, UpdateHostRequest};
  ```

with:

  ```rust
  use hennery_proto::frames::Capability;
  use hennery_proto::rest::{
      EnrollRequest, EnrollResponse, HostItem, McpAgentDelivery, PairingCodeResponse, UpdateHostRequest,
  };
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
      HostItem {
          mcp_delivery: None,
  ```

with:

  ```rust
      // Plan 8e decision E7: per agent, from the latest accepted `hello`, and
      // only for a host that takes servers at all (one without says so by
      // its capabilities). The one mapping, `McpAgentDelivery::of`.
      let mcp_delivery = record
          .mcp_isolation
          .filter(|_| record.capabilities.has(Capability::McpServers))
          .map(|isolation| {
              isolation
                  .0
                  .into_iter()
                  .map(|(agent, how)| (agent, McpAgentDelivery::of(how)))
                  .collect()
          });
      HostItem {
          mcp_delivery,
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
          }
          Err(err) => internal(err),
      }
  }
  ```

with:

  ```rust
          }
          Err(err) => internal(err),
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use hennery_kernel::operator::Operator;
      use hennery_proto::frames::{AgentIsolation, Capabilities, McpIsolation};

      fn record(capabilities: Capabilities, isolation: Option<AgentIsolation>) -> HostRecord {
          HostRecord {
              id: "h".into(),
              name: "h".into(),
              platform: "linux".into(),
              host_version: "1".into(),
              capabilities,
              default_hat_id: "hat".into(),
              workspace_roots: vec![],
              created_at: 0,
              last_seen_at: None,
              revoked_at: None,
              mcp_isolation: isolation,
          }
      }

      /// Plan 8e decision E7: per agent, through the one mapping, and only for
      /// a host that takes servers at all; none before its first `hello`.
      #[test]
      fn a_hosts_delivery_is_shown_only_when_it_takes_servers() {
          let state = AppState::new(
              crate::store::Store::open_in_memory().unwrap(),
              hennery_kernel::hosts::Hosts::open_in_memory().unwrap(),
              Operator::open_in_memory().unwrap(),
          );
          let isolation = AgentIsolation(
              [
                  ("claude".to_string(), McpIsolation::ClaudeStrict),
                  ("codex".to_string(), McpIsolation::None),
              ]
              .into(),
          );
          let takes = Capabilities(vec![Capability::McpServers]);
          let shown = host_item(&state, record(takes.clone(), Some(isolation.clone()))).mcp_delivery;
          assert_eq!(
              shown,
              Some(
                  [
                      ("claude".to_string(), McpAgentDelivery::Isolated),
                      ("codex".to_string(), McpAgentDelivery::DefaultHatOnly),
                  ]
                  .into()
              )
          );
          assert_eq!(
              host_item(&state, record(Capabilities::default(), Some(isolation))).mcp_delivery,
              None
          );
          assert_eq!(host_item(&state, record(takes, None)).mcp_delivery, None);
      }
  }
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
  pub mod push;
  mod resolve;
  ```

with:

  ```rust
  pub mod push;
  mod redact;
  mod resolve;
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      /// it deleted, and the one checkpoint its deletes owe.
      fn on_hat_purged(&self, hat_id: &str) -> anyhow::Result<()> {
          self.store.owe_checkpoint();
          let purged = hats::purge_sessions(self, hat_id).map(drop);
  ```

with:

  ```rust
      /// it deleted, then the gateway's part (lane L6, plan 8e: its tokens,
      /// stdio servers and connections, with their open streams cut), and
      /// the one checkpoint the deletes owe. Each is idempotent: a purge that
      /// stopped runs both again.
      fn on_hat_purged(&self, hat_id: &str) -> anyhow::Result<()> {
          self.store.owe_checkpoint();
          let purged = hats::purge_sessions(self, hat_id)
              .map(drop)
              .and_then(|()| self.store.purge_gateway_hat(hat_id));
  ```

Create `crates/hennery-sessions/src/redact.rs`:

  ```rust
  //! A session token the agent printed (plan 8e decision 11; the maintainer's
  //! open question Q2 of plan 8c, default chosen, reversible): what hennery
  //! stores of a session's ACP payloads, and what it forwards of them to the
  //! timeline and its stream, has every session token in it redacted (ACP
  //! core §8: tokens are never in events or SSE). The rest of the payload is
  //! kept as the host sent it (§2.3).
  //!
  //! By shape, not by value: the gateway keeps only a token's hash, so the
  //! collector cannot know the plaintext of a session's earlier tokens. Every
  //! run that starts with `hnry_session_` (in any case) and goes on with at
  //! least 8 hexadecimal digits is replaced whole, whoever's token it is: a
  //! token cut short or upper-cased is still most of a secret. What this does
  //! not catch: a token split across two updates (streamed chunks), or
  //! encoded otherwise (base64, spaced out). Those reach the timeline as the
  //! agent sent them; the token stops working at the session's next park.

  use anyhow::{Context, Result};
  use hennery_gateway::tokens::SESSION_TOKEN_PREFIX;
  use hennery_proto::frames::SessionBody;
  use serde_json::Value;

  /// What a redacted token reads as.
  pub const REDACTED: &str = "hnry_session_<redacted>";

  /// The fewest hexadecimal digits after the prefix that count as a token.
  const MIN_DIGITS: usize = 8;

  /// `text` with every token-shaped run replaced by `REDACTED`; `None` if it
  /// has none.
  pub fn text(text: &str) -> Option<String> {
      let bytes = text.as_bytes();
      let prefix = SESSION_TOKEN_PREFIX.as_bytes();
      let mut out: Option<String> = None;
      let mut copied = 0;
      let mut i = 0;
      while i + prefix.len() <= bytes.len() {
          if bytes[i..i + prefix.len()].eq_ignore_ascii_case(prefix) {
              let digits = bytes[i + prefix.len()..]
                  .iter()
                  .take_while(|b| b.is_ascii_hexdigit())
                  .count();
              if digits >= MIN_DIGITS {
                  let end = i + prefix.len() + digits;
                  let buffer = out.get_or_insert_with(|| String::with_capacity(text.len()));
                  // Both ends are on ASCII bytes: character boundaries.
                  buffer.push_str(&text[copied..i]);
                  buffer.push_str(REDACTED);
                  copied = end;
                  i = end;
                  continue;
              }
          }
          i += 1;
      }
      out.map(|mut buffer| {
          buffer.push_str(&text[copied..]);
          buffer
      })
  }

  /// `text` as it may be shown: logged, or answered to the operator.
  pub fn shown(text: &str) -> std::borrow::Cow<'_, str> {
      match self::text(text) {
          Some(redacted) => std::borrow::Cow::Owned(redacted),
          None => std::borrow::Cow::Borrowed(text),
      }
  }

  /// Redact every string of `value`, object keys too. True if any changed.
  pub fn value(value: &mut Value) -> bool {
      match value {
          Value::String(s) => match text(s) {
              Some(redacted) => {
                  *s = redacted;
                  true
              }
              None => false,
          },
          // Every item, never stopping at the first: `any` would leave the
          // rest unredacted.
          Value::Array(items) => {
              let mut changed = false;
              for item in items {
                  changed |= self::value(item);
              }
              changed
          }
          Value::Object(map) => {
              let mut changed = false;
              let keys: Vec<String> = map.keys().filter(|key| text(key).is_some()).cloned().collect();
              for key in keys {
                  if let Some(mut entry) = map.remove(&key) {
                      self::value(&mut entry);
                      map.insert(text(&key).unwrap_or(key), entry);
                      changed = true;
                  }
              }
              for entry in map.values_mut() {
                  changed |= self::value(entry);
              }
              changed
          }
          Value::Null | Value::Bool(_) | Value::Number(_) => false,
      }
  }

  /// `body` with its tokens redacted, read back as a body; `None` if it had
  /// none, which is the common case and costs one serialisation.
  pub fn body(body: &SessionBody) -> Result<Option<SessionBody>> {
      let mut json = serde_json::to_value(body)?;
      if !value(&mut json) {
          return Ok(None);
      }
      // Only strings changed, so the shape is the same; the context names no
      // content.
      serde_json::from_value(json)
          .map(Some)
          .context("a session frame with a token redacted no longer reads")
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use serde_json::json;

      fn token() -> String {
          format!("{SESSION_TOKEN_PREFIX}{}", "0a".repeat(32))
      }

      #[test]
      fn a_token_is_redacted_wherever_it_is_in_a_string() {
          let t = token();
          assert_eq!(text(&t).as_deref(), Some(REDACTED));
          assert_eq!(
              text(&format!("Bearer {t}, again {t}.")).as_deref(),
              Some(format!("Bearer {REDACTED}, again {REDACTED}.").as_str())
          );
          assert_eq!(
              text(&format!("é{t}é")).as_deref(),
              Some(format!("é{REDACTED}é").as_str())
          );
      }

      #[test]
      fn a_token_cut_short_or_upper_cased_is_redacted_too() {
          let t = token();
          assert!(text(&t[..SESSION_TOKEN_PREFIX.len() + 8]).is_some());
          assert!(text(&t.to_uppercase()).is_some());
          assert!(text(&format!("{t}ff")).is_some_and(|r| r == REDACTED), "the whole run");
      }

      #[test]
      fn what_is_not_a_token_is_left_alone() {
          for kept in [
              "",
              "hnry_session_",
              "hnry_session_0a0a0a0",
              "hnry_session_<redacted>",
              "a hnry_client_0a0a0a0a0a0a0a0a",
              "plain text",
          ] {
              assert_eq!(text(kept), None, "{kept}");
          }
      }

      #[test]
      fn every_string_of_a_value_is_redacted_keys_too() {
          let t = token();
          let mut v = json!({"a": [t, {"b": format!("x{t}")}], t.clone(): 1, "n": 2});
          assert!(value(&mut v));
          assert!(!v.to_string().contains(&t), "{v}");
          assert_eq!(v["n"], 2);
          let mut clean = json!({"a": ["b"], "c": 1});
          assert!(!value(&mut clean));
      }
  }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  use hennery_proto::frames::{
      AgentHome, AttachedSession, CollectorFrame, ConfigValue, ElicitationAction, ForgetKind, ForgetReason, Indexed,
      ParkReason, PendingKind, PendingReason, PendingResolution, SessionBody, SessionConfig, TurnOutcome,
  };
  use hennery_proto::rest::{
      AnswerRequest, AttachmentUsage, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, EventDto, HostRemovalState, PendingItem,
      PendingState, RemovalItem, RemovalState, SessionCatalog, SessionItem, SessionPage, TITLE_MAX_CHARS,
      TITLE_MAX_JSON_BYTES, TranscriptRemoval, json_char_width,
  ```

with:

  ```rust
  use hennery_gateway::revocation::Cut;
  use hennery_gateway::session::{NoSessionMcp, SessionMcp, SessionRef};
  use hennery_proto::frames::{
      AgentHome, AttachedSession, CollectorFrame, ConfigValue, ElicitationAction, ForgetKind, ForgetReason, Indexed,
      McpIsolation, McpServer, ParkReason, PendingKind, PendingReason, PendingResolution, SessionBody, SessionConfig,
      TurnOutcome,
  };
  use hennery_proto::rest::{
      AnswerRequest, AttachmentUsage, BRANCH_MAX_CHARS, BRANCH_MAX_JSON_BYTES, EventDto, HostRemovalState,
      McpSessionDelivery, McpSessionDeliveryMode, PendingItem, PendingState, RemovalItem, RemovalState, SessionCatalog,
      SessionItem, SessionPage, TITLE_MAX_CHARS, TITLE_MAX_JSON_BYTES, TranscriptRemoval, json_char_width,
      mcp_session_delivery,
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  use std::sync::{Arc, Mutex};
  ```

with:

  ```rust
  use std::sync::{Arc, Mutex, RwLock};
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  ",
  ];
  ```

with:

  ```rust
  ",
      // Plan 8e decision E10: what the session's latest start or resume was
      // given of the gateway, for its detail: the mode, how many servers,
      // when. Never a server, a header or a token. `NULL` until a start or
      // resume since plan 8e.
      "
      ALTER TABLE sessions ADD COLUMN mcp_delivery_mode TEXT;
      ALTER TABLE sessions ADD COLUMN mcp_delivery_servers INTEGER;
      ALTER TABLE sessions ADD COLUMN mcp_delivery_at TEXT;
  ",
  ];
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          config: SessionConfig,
      },
  ```

with:

  ```rust
          config: SessionConfig,
          /// Its MCP servers, with a fresh token (plan 8e).
          mcp: McpGiven,
      },
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      checkpoints: Option<Arc<Checkpoints>>,
  }
  ```

with:

  ```rust
      checkpoints: Option<Arc<Checkpoints>>,
      /// The gateway, as sessions reach it (umbrella §9, ACP core §1): handed
      /// every start's, resume's and revoke's transaction (lane L1). None
      /// (`NoSessionMcp` stands in) until the collector sets it
      /// (`set_session_mcp`).
      mcp: RwLock<Option<Arc<dyn SessionMcp>>>,
  }

  /// What the start or resume route read of the host for the delivery
  /// decision (umbrella §8.5, lane L2), outside the store: from the hub, the
  /// host's live connection; from the kernel, whether its rules name another
  /// hat. The rest (the host's default hat, its live sessions' hats) is read
  /// in the transition's own transaction.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct McpContext {
      /// The connection announced `mcp_servers`.
      pub capable: bool,
      /// How it isolates the session's agent.
      pub isolation: McpIsolation,
      /// `Hosts::rules_name_other_hats`.
      pub rules_name_other_hats: bool,
  }

  impl McpContext {
      /// A host that takes no servers: what a store without a gateway, or a
      /// host that is not connected, decides.
      pub const NONE: Self = Self {
          capable: false,
          isolation: McpIsolation::None,
          rules_name_other_hats: false,
      };
  }

  /// What a start or resume was given (plan 8e): its servers, for its frame
  /// and nowhere else, and the mode, for `isolation_waived`.
  #[derive(Clone, PartialEq)]
  pub struct McpGiven {
      pub mode: McpSessionDeliveryMode,
      pub servers: Vec<McpServer>,
  }

  impl McpGiven {
      /// The frame's MCP part: `isolation_waived` only for the default hat of
      /// a host that cannot isolate the agent (plan 8c's hand-off).
      pub fn frame(self) -> hennery_proto::frames::McpDelivery {
          hennery_proto::frames::McpDelivery {
              isolation_waived: self.mode == McpSessionDeliveryMode::Unisolated,
              mcp_servers: self.servers,
          }
      }
  }

  // By hand: the servers carry the session's token and stdio values.
  impl std::fmt::Debug for McpGiven {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.debug_struct("McpGiven")
              .field("mode", &self.mode)
              .field("servers", &self.servers)
              .finish()
      }
  }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
  /// `Store::close_now`'s body, inside the caller's transaction. A tombstone
  /// is left alone (plan 9a A1).
  fn close_in(tx: &Transaction<'_>, owner: &str, session_id: &str) -> Result<Vec<EventDto>> {
  ```

with:

  ```rust
  /// `Store::close_now`'s body, inside the caller's transaction, with the
  /// session's token revoked there too (lane L4): the cut is the caller's to
  /// make once it commits. A tombstone is left alone (plan 9a A1).
  fn close_in(tx: &Transaction<'_>, owner: &str, mcp: &dyn SessionMcp, session_id: &str) -> Result<(Vec<EventDto>, Cut)> {
      let events = close_session_in(tx, owner, session_id)?;
      // Whether or not this closed it: a closed session's token is revoked
      // already, so this is a no-op then, never a fresh token's revoke (a
      // resume moves the row to `starting` first, in its own transaction).
      let cut = mcp.revoke_in(tx, session_id)?;
      Ok((events, cut))
  }

  /// Closes the session, if it is not closed already: the events that wrote.
  fn close_session_in(tx: &Transaction<'_>, owner: &str, session_id: &str) -> Result<Vec<EventDto>> {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              checkpoints,
          })
      }
  ```

with:

  ```rust
              checkpoints,
              mcp: RwLock::new(None),
          })
      }

      /// The gateway, on the same `hennery.db` (the collector sets it before
      /// it serves; plan 8e).
      pub fn set_session_mcp(&self, mcp: Arc<dyn SessionMcp>) {
          *self.mcp.write().expect("session mcp lock") = Some(mcp);
      }

      /// Whether the gateway is set (the collector's wiring test).
      pub fn has_session_mcp(&self) -> bool {
          self.mcp.read().expect("session mcp lock").is_some()
      }

      /// The gateway's part of a hat's purge (lane L6): through the gateway
      /// this store was given, so a store without one has none to purge.
      pub fn purge_gateway_hat(&self, hat_id: &str) -> Result<()> {
          self.mcp().purge_hat(hat_id)
      }

      /// The gateway, or the stand-in that gives nothing.
      pub(crate) fn mcp(&self) -> Arc<dyn SessionMcp> {
          self.mcp
              .read()
              .expect("session mcp lock")
              .clone()
              .unwrap_or_else(|| Arc::new(NoSessionMcp))
      }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// A new session, `starting`, in `hat_id` as `rule_id` decided (none:
      /// its host's default hat). `cwd` is canonical on its host. `false`,
      /// and nothing stored, if the hat is frozen for its purge: the start
      /// resolved its hat before the freeze (plan 9c decision 10c). One
      /// statement, so the check and the insert cannot be told apart.
  ```

with:

  ```rust
      /// `create_session_with_mcp` for a host that takes no MCP servers: what
      /// the store's own tests start. `false` where that answers `None`.
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let ts = now();
          let created = self.conn().execute(
  ```

with:

  ```rust
          Ok(self
              .create_session_with_mcp(id, host_id, agent, cwd, hat_id, rule_id, McpContext::NONE)?
              .is_some())
      }

      /// A new session, `starting`, in `hat_id` as `rule_id` decided (none:
      /// its host's default hat). `cwd` is canonical on its host. `None`, and
      /// nothing stored, if the hat is frozen for its purge: the start
      /// resolved its hat before the freeze (plan 9c decision 10c); the check
      /// and the insert are one statement. In the same transaction (lane L1):
      /// the delivery decision and the session's MCP servers, its token
      /// minted, so a failed mint stores no session.
      #[allow(clippy::too_many_arguments)]
      pub fn create_session_with_mcp(
          &self,
          id: &str,
          host_id: &str,
          agent: &str,
          cwd: &str,
          hat_id: &str,
          rule_id: Option<&str>,
          mcp: McpContext,
      ) -> Result<Option<McpGiven>> {
          let ts = now();
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let created = tx.execute(
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          Ok(created == 1)
      }

      /// Fail a session's start; a tombstone is left alone (plan 9a A1).
      pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
          self.conn().execute(
  ```

with:

  ```rust
          if created == 0 {
              return Ok(None);
          }
          // A fresh id had no token before this one: nothing to cut.
          let (given, _superseded) = self.deliver_in(&tx, id, host_id, hat_id, mcp, &ts)?;
          tx.commit()?;
          Ok(Some(given))
      }

      /// The delivery decision (umbrella §8.5, lane L2) and the servers it
      /// gives, inside a start's or resume's transaction, recorded on the
      /// session (decision E10). The session is `starting` already, so it
      /// counts itself in the mixedness: a session outside the default hat
      /// always finds its host mixed.
      fn deliver_in(
          &self,
          tx: &Transaction<'_>,
          session_id: &str,
          host_id: &str,
          hat_id: &str,
          mcp: McpContext,
          ts: &str,
      ) -> Result<(McpGiven, Cut)> {
          // A host not in the registry (the store's own tests) has no default
          // hat: then no session is in it.
          let default_hat: String = tx
              .query_row(
                  "SELECT default_hat_id FROM hosts WHERE id = ?1 AND owner_id = ?2",
                  [host_id, &self.owner],
                  |r| r.get(0),
              )
              .optional()?
              .unwrap_or_default();
          // One query on `sessions.hat_id` (lane L2): a live session of
          // another hat than the default, this one included.
          let other_hat_live: bool = tx.query_row(
              "SELECT EXISTS (SELECT 1 FROM sessions
                   WHERE host_id = ?1 AND owner_id = ?2 AND hat_id <> ?3
                       AND (lifecycle IN ('starting', 'active') OR presumed_parked = 1))",
              params![host_id, self.owner, default_hat],
              |r| r.get(0),
          )?;
          let mixed = mcp.rules_name_other_hats || other_hat_live;
          let mode = mcp_session_delivery(mcp.capable, mcp.isolation, mixed, hat_id == default_hat);
          let delivered = self.mcp().servers_in(
              tx,
              SessionRef {
                  session_id,
                  host_id,
                  hat_id,
              },
              mode,
          )?;
          tx.execute(
              "UPDATE sessions SET mcp_delivery_mode = ?2, mcp_delivery_servers = ?3, mcp_delivery_at = ?4
               WHERE id = ?1 AND owner_id = ?5",
              params![
                  session_id,
                  mode.as_str(),
                  delivered.servers.len() as i64,
                  ts,
                  self.owner
              ],
          )?;
          Ok((
              McpGiven {
                  mode,
                  servers: delivered.servers,
              },
              delivered.cut,
          ))
      }

      /// What the session's latest start or resume was given (decision
      /// E10); `None` before one since plan 8e.
      pub fn mcp_delivery(&self, id: &str) -> Result<Option<McpSessionDelivery>> {
          let row: Option<(Option<String>, Option<i64>, Option<String>)> = self
              .conn()
              .query_row(
                  "SELECT mcp_delivery_mode, mcp_delivery_servers, mcp_delivery_at FROM sessions
                   WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?2",
                  [id, &self.owner],
                  |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
              )
              .optional()?;
          Ok(match row {
              Some((Some(mode), Some(servers), Some(at))) => Some(McpSessionDelivery {
                  mode: McpSessionDeliveryMode::parse(&mode).context("a stored delivery mode")?,
                  servers: u32::try_from(servers).context("a stored server count")?,
                  at,
              }),
              _ => None,
          })
      }

      /// Fail a session's start; a tombstone is left alone (plan 9a A1). Its
      /// token goes with it (lane L4).
      pub fn mark_failed(&self, id: &str, reason: &str) -> Result<()> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let failed = tx.execute(
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
               WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?3",
              params![id, reason, self.owner],
          )?;
          Ok(())
      }

  ```

with:

  ```rust
               WHERE id = ?1 AND lifecycle <> 'deleted' AND owner_id = ?3",
              params![id, reason, self.owner],
          )?;
          let cut = if failed == 1 {
              self.mcp().revoke_in(&tx, id)?
          } else {
              Cut::default()
          };
          tx.commit()?;
          self.mcp().cut(cut);
          Ok(())
      }

  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// session still `starting` is failed. Whatever moved it on while the
      /// request was out (its `session_started`, a close, a newer resume's
      /// outcome) is left as it is.
      pub fn mark_failed_if_starting(&self, id: &str, reason: &str) -> Result<()> {
          self.conn().execute(
  ```

with:

  ```rust
      /// session still `starting` is failed, and only its token revoked.
      /// Whatever moved it on while the request was out (its
      /// `session_started`, a close, a newer resume's outcome) is left as it
      /// is.
      pub fn mark_failed_if_starting(&self, id: &str, reason: &str) -> Result<()> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let failed = tx.execute(
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              params![id, reason, self.owner],
          )?;
          Ok(())
      }
  ```

with:

  ```rust
              params![id, reason, self.owner],
          )?;
          let cut = if failed == 1 {
              self.mcp().revoke_in(&tx, id)?
          } else {
              Cut::default()
          };
          tx.commit()?;
          self.mcp().cut(cut);
          Ok(())
      }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          };
          let mut unconfirmed = false;
  ```

with:

  ```rust
          };
          let mcp = self.mcp();
          let mut cut = Cut::default();
          let mut unconfirmed = false;
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      close_in(&tx, &self.owner, session_id)?;
  ```

with:

  ```rust
                      cut = close_in(&tx, &self.owner, mcp.as_ref(), session_id)?.1;
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                   open_turn_id = NULL, activity = NULL, presumed_parked = 0, close_requested = 0, agent_home = NULL
  ```

with:

  ```rust
                   open_turn_id = NULL, activity = NULL, presumed_parked = 0, close_requested = 0, agent_home = NULL,
                   mcp_delivery_mode = NULL, mcp_delivery_servers = NULL, mcp_delivery_at = NULL
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          // plan 8: revoke the session's gateway tokens here
  ```

with:

  ```rust
          // Its token, in this transaction (lane L4; the purge lane's marker):
          // a parked or failed session's is revoked already, a no-op then.
          let cut = cut.and(mcp.revoke_in(&tx, session_id)?);
          tx.commit()?;
          mcp.cut(cut);
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let events = close_in(&tx, &self.owner, session_id)?;
          tx.commit()?;
  ```

with:

  ```rust
          let mcp = self.mcp();
          let (events, cut) = close_in(&tx, &self.owner, mcp.as_ref(), session_id)?;
          tx.commit()?;
          mcp.cut(cut);
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let events = if still_requested {
              close_in(&tx, &self.owner, session_id)?
          } else {
              Vec::new()
          };
          tx.commit()?;
          Ok(events)
  ```

with:

  ```rust
          let mcp = self.mcp();
          let (events, cut) = if still_requested {
              close_in(&tx, &self.owner, mcp.as_ref(), session_id)?
          } else {
              (Vec::new(), Cut::default())
          };
          tx.commit()?;
          mcp.cut(cut);
          Ok(events)
      }

      /// `request_resume_with_mcp` for a host that takes no MCP servers: what
      /// the store's own tests resume.
      pub fn request_resume(&self, session_id: &str, hat_id: &str) -> Result<ResumeRequest> {
          self.request_resume_with_mcp(session_id, hat_id, McpContext::NONE)
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      /// first.
      pub fn request_resume(&self, session_id: &str, hat_id: &str) -> Result<ResumeRequest> {
  ```

with:

  ```rust
      /// first. In the same transaction (lane L1): the delivery decision and
      /// the session's MCP servers, a fresh token minted that supersedes the
      /// one before (ACP core §4.3), so a failed mint leaves the session as
      /// it was.
      pub fn request_resume_with_mcp(&self, session_id: &str, hat_id: &str, mcp: McpContext) -> Result<ResumeRequest> {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          )?;
          tx.commit()?;
          Ok(ResumeRequest::Starting {
  ```

with:

  ```rust
          )?;
          let host_id: String = tx.query_row(
              "SELECT host_id FROM sessions WHERE id = ?1 AND owner_id = ?2",
              [session_id, &self.owner],
              |r| r.get(0),
          )?;
          let config = stored_config(config)?;
          let (given, cut) = self.deliver_in(&tx, session_id, &host_id, &stored_hat, mcp, &ts)?;
          tx.commit()?;
          self.mcp().cut(cut);
          Ok(ResumeRequest::Starting {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              config: stored_config(config)?,
  ```

with:

  ```rust
              config,
              mcp: given,
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          )?;
          tx.commit()?;
          Ok(Reassign::Done(event))
  ```

with:

  ```rust
          )?;
          // A token's hat is fixed at its mint (plan 8d's O8): one still live
          // would reach the old hat's connections. No adapter runs, so none
          // should be; revoked here whatever a missed revoke left (lane L4).
          let mcp = self.mcp();
          let cut = mcp.revoke_in(&tx, session_id)?;
          tx.commit()?;
          mcp.cut(cut);
          Ok(Reassign::Done(event))
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          )?;
          tx.commit()?;
          Ok(events)
  ```

with:

  ```rust
          )?;
          // Every token of the host, in this transaction (ACP core §4.8, lane
          // L4): the proxy refuses a revoked host's tokens by its join anyway,
          // but a token row must not outlive its host's revoke as live.
          let mcp = self.mcp();
          let cut = mcp.revoke_host_in(&tx, host_id)?;
          tx.commit()?;
          mcp.cut(cut);
          Ok(events)
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
      pub fn ingest_fact(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Ingested> {
          let mut conn = self.conn();
  ```

with:

  ```rust
      pub fn ingest_fact(&self, session_id: &str, seq: u64, body: &SessionBody) -> Result<Ingested> {
          // Before anything reads it (plan 8e decision 11): a session token
          // the agent printed is stored, compared, extracted and published
          // only redacted.
          let redacted = crate::redact::body(body)?;
          let body = redacted.as_ref().unwrap_or(body);
          let mcp = self.mcp();
          let mut cut = Cut::default();
          let mut conn = self.conn();
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                  if changed == 0 {
                      created.clear();
                      mark_unapplied(&tx, &self.owner, fact_id)?;
                  }
              }
              SessionBody::TurnStarted { turn_id, .. } => {
  ```

with:

  ```rust
                  if changed == 0 {
                      created.clear();
                      mark_unapplied(&tx, &self.owner, fact_id)?;
                  } else {
                      // The start that failed holds its token no more (lane
                      // L4). Only when it applied: a late one must not end a
                      // resume's fresh token.
                      cut = mcp.revoke_in(&tx, session_id)?;
                  }
              }
              SessionBody::TurnStarted { turn_id, .. } => {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      created.extend(cancel_open_pending(&tx, &self.owner, session_id, reason, &ts)?);
                  }
  ```

with:

  ```rust
                      created.extend(cancel_open_pending(&tx, &self.owner, session_id, reason, &ts)?);
                      // Detached: its token is done (lane L4).
                      cut = mcp.revoke_in(&tx, session_id)?;
                  }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      )?);
                  }
  ```

with:

  ```rust
                      )?);
                      // Closed: its token is done (lane L4).
                      cut = mcp.revoke_in(&tx, session_id)?;
                  }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              SessionBody::AdapterExited { .. } | SessionBody::HostNote { .. } => {
  ```

with:

  ```rust
              SessionBody::AdapterExited { .. } => {
                  if !fact_applies(&tx, &self.owner, session_id, None)? {
                      created.clear();
                      mark_unapplied(&tx, &self.owner, fact_id)?;
                  } else {
                      // The adapter of an attached session is gone, and its
                      // token with it (lane L4), ahead of the `session_parked`
                      // that follows. Not a `starting` one: its start's own
                      // `start_failed` revokes, and a resume's fresh token is
                      // not this adapter's.
                      let attached: bool = tx.query_row(
                          "SELECT lifecycle = 'active' OR presumed_parked = 1 FROM sessions
                           WHERE id = ?1 AND owner_id = ?2",
                          [session_id, &self.owner],
                          |r| r.get(0),
                      )?;
                      if attached {
                          cut = mcp.revoke_in(&tx, session_id)?;
                      }
                  }
              }
              SessionBody::HostNote { .. } => {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          Ok(Ingested { events: created, edge })
  ```

with:

  ```rust
          tx.commit()?;
          mcp.cut(cut);
          Ok(Ingested { events: created, edge })
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let mut out = Reconciliation::default();
          for (id, lifecycle, open_turn, close_requested, presumed) in rows {
  ```

with:

  ```rust
          let mut out = Reconciliation::default();
          let mcp = self.mcp();
          let mut cut = Cut::default();
          for (id, lifecycle, open_turn, close_requested, presumed) in rows {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                      )?;
                  }
  ```

with:

  ```rust
                      )?;
                      // Never started: its token goes with it (lane L4).
                      cut = cut.and(mcp.revoke_in(&tx, &id)?);
                  }
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
                          )?;
                      } else if close_requested {
  ```

with:

  ```rust
                          )?;
                          // The restarted host runs no adapter of it (lane L4).
                          cut = cut.and(mcp.revoke_in(&tx, &id)?);
                      } else if close_requested {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          Ok(out)
  ```

with:

  ```rust
          tx.commit()?;
          mcp.cut(cut);
          Ok(out)
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
          .register(&host_id, tx.clone(), capabilities.clone(), mcp_isolation)
  ```

with:

  ```rust
          .register(&host_id, tx.clone(), capabilities.clone(), mcp_isolation.clone())
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
          tracing::warn!(%host_id, error = %err, "recording the host's hello failed");
      }
  ```

with:

  ```rust
          tracing::warn!(%host_id, error = %err, "recording the host's hello failed");
      }
      // For the host list while it is away (plan 8e decision E7).
      if let Err(err) = state.hosts.record_mcp_isolation(&host_id, &mcp_isolation) {
          tracing::warn!(%host_id, error = %err, "recording the host's MCP isolation failed");
      }
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
              Err(err) => {
                  tracing::warn!(%host_id, error = %err, "ignoring unknown or invalid frame");
  ```

with:

  ```rust
              // By the error's kind and place only: its text can quote a
              // string of the frame, a token the agent printed say (plan 8e,
              // as plan 8c did on the host).
              Err(err) => {
                  tracing::warn!(
                      %host_id,
                      kind = ?err.classify(),
                      line = err.line(),
                      column = err.column(),
                      "ignoring unknown or invalid frame"
                  );
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                          tracing::warn!(%host_id, %session_id, %code, %message, "reconcile close_session rejected");
  ```

with:

  ```rust
                          tracing::warn!(%host_id, %session_id, %code, message = %crate::redact::shown(&message), "reconcile close_session rejected");
  ```

In `crates/hennery-sessions/src/ws.rs`, replace:

  ```rust
                          tracing::warn!(%host_id, %request_id, %code, %message, "host refused a request nobody waits for");
  ```

with:

  ```rust
                          tracing::warn!(%host_id, %request_id, %code, message = %crate::redact::shown(&message), "host refused a request nobody waits for");
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
      let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
      // The gateway's proxy (plan 8d), `/mcp/<slug>`: bearer tokens, beside
      // the operator's routes and outside them (lane L8), sending only
      // through the kernel's egress policy, the collector's one `Egress`
      // above, shared with Web Push.
      let proxy = hennery_gateway::proxy::ProxyState::full(
          std::sync::Arc::new(hennery_gateway::scope::ProxyStore::open(&db)?),
          &gateway,
          egress.clone(),
          hennery_gateway::proxy::Limits::default(),
      );
  ```

with:

  ```rust
      let gateway_routes = gateway(&state, &db, &keys, &egress)?;
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
          hennery_sessions::router(state.clone())
              .merge(hennery_gateway::api::router(gateway))
              .merge(hennery_gateway::proxy::router(proxy)),
  ```

with:

  ```rust
          hennery_sessions::router(state.clone()).merge(gateway_routes),
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust

  /// Web Push delivery (plan 10b-ii): the state's notices go to a task that
  ```

with:

  ```rust

  /// The gateway on the collector's `hennery.db` (plan 8a), wired into the
  /// sessions `state` serves, and its routes. Sessions mint and revoke their
  /// tokens through it, inside their own transactions (plan 8e, lane L1);
  /// its proxy (plan 8d), `/mcp/<slug>`, watches the same revocations, so a
  /// revoke cuts what is open on the token. The proxy takes bearer tokens,
  /// beside the operator's routes and outside them (lane L8), and sends only
  /// through `egress`, the collector's one, shared with Web Push.
  fn gateway(
      state: &AppState,
      db: &std::path::Path,
      keys: &hennery_gateway::key::KeySource,
      egress: &hennery_kernel::egress::Egress,
  ) -> Result<axum::Router> {
      let gateway = hennery_gateway::open(db, keys, state.operator.clone())?;
      state
          .store
          .set_session_mcp(std::sync::Arc::new(hennery_gateway::session::GatewayMcp::new(&gateway)));
      let proxy = hennery_gateway::proxy::ProxyState::full(
          std::sync::Arc::new(hennery_gateway::scope::ProxyStore::open(db)?),
          &gateway,
          egress.clone(),
          hennery_gateway::proxy::Limits::default(),
      );
      Ok(hennery_gateway::api::router(gateway).merge(hennery_gateway::proxy::router(proxy)))
  }

  /// Web Push delivery (plan 10b-ii): the state's notices go to a task that
  ```

In `crates/hennery/src/main.rs`, replace:

  ```rust
      use super::*;

  ```

with:

  ```rust
      use super::*;

      /// Plan 8e: the collector gives its sessions the gateway (lane L1), or
      /// no session would ever get a server (plan 8c's release blocker, lane
      /// L15). The same function `run_collector` calls.
      #[test]
      fn the_collector_gives_its_sessions_the_gateway() {
          let dir = tempfile::tempdir().unwrap();
          let db = dir.path().join("hennery.db");
          let state = AppState::new(
              Store::open(&db).unwrap(),
              Hosts::open(&db).unwrap(),
              Operator::open(&db).unwrap(),
          );
          assert!(!state.store.has_session_mcp());
          let keys = hennery_gateway::key::KeySource::from_vars(dir.path(), Some("07".repeat(32).into()), None).unwrap();
          let egress = hennery_kernel::egress::Egress::new(hennery_kernel::egress::Timeouts::DEFAULT).unwrap();
          let _routes = gateway(&state, &db, &keys, &egress).unwrap();
          assert!(state.store.has_session_mcp());
      }

  ```

- [ ] **Step 4: Run the checks**

   Let cargo add `hennery-gateway` to the sessions' entry and `axum` to the testkit's in `Cargo.lock`, then run the five checks:

   ```sh
   nix develop -c cargo check --workspace --all-targets
   ```

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "feat(sessions): sessions get their hat's MCP servers through the gateway, tokens revoked at every site"
   ```

### Task 5: An upstream's session id bound to its token, a deadline on JSON answers

**Files:** Create `crates/hennery-gateway/src/session_id.rs`, `tests/session_ids.rs`; modify the workspace `Cargo.toml` (`hmac = "=0.12.1"`, already in the lock), the gateway's `Cargo.toml`, `src/lib.rs`, `src/proxy.rs`, `tests/proxy.rs`, `tests/differential.rs`, `tests/support/upstream.rs`.
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
  /// upstream session, open their streams at once; each sees only its own.
  #[tokio::test]
  ```

with:

  ```rust
  /// upstream session, open their streams at once; each sees only its own.
  /// Each gets its session id from a `POST`, wrapped for its token (plan 8e
  /// decision 13), and the upstream only ever sees the bare ids.
  #[tokio::test]
  ```

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
      let other = s.h.mint("s2", "host-a", &hat);
      // Each event its own chunk, a pause between them, so the two streams
  ```

with:

  ```rust
      let other = s.h.mint("s2", "host-a", &hat);
      let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      let theirs = s.h.session_id(&s.upstream, "linear", &other, "up-2").await;
      // Each event its own chunk, a pause between them, so the two streams
  ```

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
      let open = |token: String, upstream_session: &'static str| {
  ```

with:

  ```rust
      let open = |token: String, session: String, upstream_session: &'static str| {
  ```

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
                  .header("mcp-session-id", upstream_session)
                  .send()
                  .await
                  .unwrap();
              assert_eq!(
                  resp.headers()["mcp-session-id"],
                  format!("srv-{upstream_session}").as_str()
              );
              resp.text().await.unwrap()
          }
      };
      let (one, two) = tokio::join!(open(s.token.clone(), "up-1"), open(other, "up-2"));
  ```

with:

  ```rust
                  .header("mcp-session-id", session)
                  .send()
                  .await
                  .unwrap();
              let id = resp.headers()["mcp-session-id"].to_str().unwrap().to_owned();
              assert!(id.starts_with(&format!("srv-{upstream_session}.")), "{id}");
              resp.text().await.unwrap()
          }
      };
      let (one, two) = tokio::join!(open(s.token.clone(), mine, "up-1"), open(other, theirs, "up-2"));
      let streams: Vec<_> = s
          .upstream
          .seen()
          .into_iter()
          .filter(|seen| seen.method == "GET")
          .collect();
      assert_eq!(streams.len(), 2);
      let mut bare: Vec<_> = streams
          .iter()
          .map(|seen| seen.header("mcp-session-id").unwrap())
          .collect();
      bare.sort();
      assert_eq!(bare, ["up-1", "up-2"], "the upstream sees the bare ids only");
  ```

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
              assert_eq!(message["params"]["for"], mine, "{event}");
          }
      }
  }

  /// A server request inside a JSON answer (gateway spec §5.6, plan
  ```

with:

  ```rust
              assert_eq!(message["params"]["for"], mine, "{event}");
          }
      }
  }

  /// Session ids as a client or an intermediary may spell them (plan 8e
  /// decision 13): another token's id, joined to this token's with a comma
  /// either way round (how a list header is folded), or wrapped once more.
  /// Each is the same 404, and nothing goes up.
  #[tokio::test]
  async fn no_spelling_of_another_tokens_session_id_goes_up() {
      let s = setup().await;
      let other = s.h.mint("s2", "host-a", &s.h.hat());
      let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      let theirs = s.h.session_id(&s.upstream, "linear", &other, "up-2").await;
      let before = s.upstream.seen().len();
      for spelling in [
          theirs.clone(),
          format!("{mine},{theirs}"),
          format!("{theirs}, {mine}"),
          format!("up-1.{mine}"),
          format!("{theirs}.{}", &mine["up-1.".len()..]),
      ] {
          let resp =
              s.h.client
                  .post(s.h.url("linear"))
                  .bearer_auth(&s.token)
                  .header(header::CONTENT_TYPE, "application/json")
                  .header("mcp-session-id", &spelling)
                  .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
                  .send()
                  .await
                  .unwrap();
          assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{spelling}");
      }
      assert_eq!(s.upstream.seen().len(), before, "nothing went up");
  }

  /// A server request inside a JSON answer (gateway spec §5.6, plan
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
      let s = setup(CredKind::Static, None).await;
      s.upstream.reply(|_, _| {
  ```

with:

  ```rust
      let s = setup(CredKind::Static, None).await;
      let session =
          s.h.session_id(&s.upstream, "linear", &s.token, "upstream-session-1")
              .await;
      s.upstream.reply(|_, _| {
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
              .header(header::ACCEPT, "application/json, text/event-stream")
              .header("mcp-session-id", "upstream-session-1")
              .header("mcp-protocol-version", "2025-06-18")
  ```

with:

  ```rust
              .header(header::ACCEPT, "application/json, text/event-stream")
              .header("mcp-session-id", &session)
              .header("mcp-protocol-version", "2025-06-18")
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
      assert_eq!(headers["mcp-session-id"], "upstream-session-1");
  ```

with:

  ```rust
      // Wrapped for this token and connection (plan 8e decision 13): the
      // same upstream id, the same wrapped id.
      assert_eq!(headers["mcp-session-id"], session.as_str());
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
      assert_eq!(seen.len(), 1);
      let up = &seen[0];
  ```

with:

  ```rust
      assert_eq!(seen.len(), 2, "the session's ping, then the request");
      let up = &seen[1];
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
      // A `DELETE` ends the upstream session (G-16): forwarded, 204 back.
  ```

with:

  ```rust
      // A `DELETE` ends the upstream session (G-16): forwarded, 204 back,
      // its id unwrapped (plan 8e decision 13).
      let session =
          s.h.session_id(&s.upstream, "linear", &s.token, "upstream-session-9")
              .await;
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
              .header("mcp-session-id", "upstream-session-9")
  ```

with:

  ```rust
              .header("mcp-session-id", &session)
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust

  /// Plan 8d decision 10: the response head has `head_timeout` to arrive (300
  ```

with:

  ```rust

  /// Spaces of JSON whitespace, one every 50 ms: `Some(n)` of them, then a
  /// JSON-RPC result; `None`, forever. An upstream that trickles its answer.
  fn trickle(spaces: Option<usize>) -> Response {
      let body = futures::stream::unfold(0, move |n| async move {
          let chunk: &'static [u8] = match spaces {
              Some(spaces) if n > spaces => return None,
              Some(spaces) if n == spaces => br#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
              _ => b" ",
          };
          tokio::time::sleep(Duration::from_millis(50)).await;
          Some((Ok::<_, std::io::Error>(axum::body::Bytes::from_static(chunk)), n + 1))
      });
      Response::builder()
          .header(header::CONTENT_TYPE, "application/json")
          .body(Body::from_stream(body))
          .unwrap()
  }

  /// Plan 8e decision 14: a JSON answer, read whole before any of it goes
  /// down, has `answer_timeout` from its head to arrive. One that trickles
  /// and never ends is 502 `upstream_unreachable` then, not when the client
  /// gives up, and its request permit is free again; one that trickles and
  /// ends in time passes.
  #[tokio::test]
  async fn a_json_answer_that_trickles_past_the_answer_timeout_is_502() {
      let deadline = Duration::from_secs(2);
      let limits = Limits::new(1, 8, Duration::from_secs(10), Duration::from_secs(2)).with_answer_timeout(deadline);
      let s = setup_with(Harness::with_limits(limits).await, CredKind::None, None).await;
      s.upstream.reply(|_, _| trickle(None));
      let started = std::time::Instant::now();
      let resp = tokio::time::timeout(Duration::from_secs(10), s.h.post("linear", &s.token, &ping(1)))
          .await
          .expect("no answer within 10 s: the answer timeout did not fire");
      assert!(started.elapsed() >= deadline, "{:?}", started.elapsed());
      assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
      let body: Value = resp.json().await.unwrap();
      assert_eq!(body["code"], "upstream_unreachable");
      // The one request permit is free: a trickle that ends in time passes.
      s.upstream.reply(|_, _| trickle(Some(6)));
      let resp = s.h.post("linear", &s.token, &ping(1)).await;
      assert_eq!(resp.status(), StatusCode::OK);
      let body: Value = resp.json().await.unwrap();
      assert_eq!(body["id"], 1);
  }

  /// Plan 8d decision 10: the response head has `head_timeout` to arrive (300
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
          .await;
          let sampling =
  ```

with:

  ```rust
          .await;
          let session =
              s.h.session_id(&s.upstream, "linear", &s.token, "client-session-9")
                  .await;
          let sampling =
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust
                  .header("mcp-session-id", "client-session-9")
  ```

with:

  ```rust
                  .header("mcp-session-id", &session)
  ```

Create `crates/hennery-gateway/tests/session_ids.rs`:

  ```rust
  //! An upstream's `Mcp-Session-Id`, bound to the token that opened it (plan
  //! 8e decision 13; gateway spec §5.2, §5.5). Every token on a connection
  //! sends upstream with the connection's one credential, so a session id
  //! that is not this token's on this connection is refused with the same
  //! 404 as an unknown token, and nothing goes up.

  mod support;

  use axum::body::Body;
  use axum::http::{StatusCode, header};
  use axum::response::Response;
  use hennery_gateway::model::CredKind;
  use reqwest::Method;
  use serde_json::{Value, json};
  use support::upstream::{FakeUpstream, Harness, json};

  struct Setup {
      h: Harness,
      upstream: FakeUpstream,
      token: String,
      /// Another session's token, on the same host, hat and connection.
      other: String,
  }

  async fn setup() -> Setup {
      let h = Harness::new().await;
      let upstream = FakeUpstream::start().await;
      h.host("host-a", 1);
      let hat = h.hat();
      let id = h.connection_in("linear", &upstream.url("/mcp"), CredKind::None, &hat, None);
      h.mount(&id, &["host-a"]);
      let token = h.mint("s1", "host-a", &hat);
      let other = h.mint("s2", "host-a", &hat);
      Setup {
          h,
          upstream,
          token,
          other,
      }
  }

  fn ping() -> Value {
      json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })
  }

  /// `method` on `slug` with `token` and the session ids `ids`.
  async fn send(h: &Harness, method: Method, slug: &str, token: &str, ids: &[&str]) -> reqwest::Response {
      let mut request = h
          .client
          .request(method.clone(), h.url(slug))
          .bearer_auth(token)
          .header(header::ACCEPT, "application/json, text/event-stream");
      for id in ids {
          request = request.header("mcp-session-id", *id);
      }
      if method == Method::POST {
          request = request
              .header(header::CONTENT_TYPE, "application/json")
              .body(ping().to_string());
      }
      request.send().await.unwrap()
  }

  async fn assert_refused(resp: reqwest::Response, what: &str) {
      assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{what}");
      let body: Value = resp.json().await.unwrap();
      assert_eq!(body["code"], "not_found", "{what}");
  }

  /// The id comes down wrapped and goes up bare; a request with none goes up
  /// with none.
  #[tokio::test]
  async fn a_session_id_comes_down_wrapped_and_goes_up_bare() {
      let s = setup().await;
      let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      assert!(s.upstream.seen()[0].header("mcp-session-id").is_none());
      for method in [Method::POST, Method::GET, Method::DELETE] {
          let resp = send(&s.h, method.clone(), "linear", &s.token, &[&mine]).await;
          assert_eq!(resp.status(), StatusCode::OK, "{method}");
          let up = s.upstream.seen().pop().unwrap();
          assert_eq!(up.method, method.as_str());
          assert_eq!(up.header("mcp-session-id"), Some("up-1"), "{method}");
      }
  }

  /// The finding #99 left open: another session's token, on the same
  /// connection and credential, cannot ride this session's upstream session.
  #[tokio::test]
  async fn another_tokens_session_id_is_404_and_nothing_goes_up() {
      let s = setup().await;
      let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      let before = s.upstream.seen().len();
      for method in [Method::POST, Method::GET, Method::DELETE] {
          let resp = send(&s.h, method.clone(), "linear", &s.other, &[&mine]).await;
          assert_refused(resp, method.as_str()).await;
      }
      assert_eq!(s.upstream.seen().len(), before, "nothing went up");
  }

  /// Bound to the connection too: the id one connection gave is no id on
  /// another, under the same token.
  #[tokio::test]
  async fn a_session_id_from_another_connection_is_404() {
      let s = setup().await;
      let second = FakeUpstream::start().await;
      let id =
          s.h.connection_in("github", &second.url("/mcp"), CredKind::None, &s.h.hat(), None);
      s.h.mount(&id, &["host-a"]);
      let linear = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      assert_refused(
          send(&s.h, Method::POST, "github", &s.token, &[&linear]).await,
          "linear's id on github",
      )
      .await;
      assert!(second.seen().is_empty(), "nothing went up");
  }

  /// A bare upstream id, a forged or respelled tag, or two ids: the same 404,
  /// nothing up.
  #[tokio::test]
  async fn a_bare_forged_or_doubled_session_id_is_404() {
      let s = setup().await;
      let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      let before = s.upstream.seen().len();
      let tag = &mine["up-1.".len()..];
      let forged = format!("up-1.{}", "0".repeat(64));
      let upper = format!("up-1.{}", tag.to_uppercase());
      let moved = format!("up-2.{tag}");
      for (ids, what) in [
          (vec!["up-1"], "bare"),
          (vec![forged.as_str()], "forged"),
          (vec![upper.as_str()], "respelled"),
          (vec![moved.as_str()], "another upstream id"),
          (vec![mine.as_str(), mine.as_str()], "twice"),
      ] {
          assert_refused(send(&s.h, Method::POST, "linear", &s.token, &ids).await, what).await;
      }
      assert_eq!(s.upstream.seen().len(), before, "nothing went up");
  }

  /// The key is the process's own: after a restart every id is refused, and
  /// the client initializes again (MCP: a 404 on a request with a session id).
  #[tokio::test]
  async fn a_restarted_proxy_refuses_the_ids_it_gave_before() {
      let mut s = setup().await;
      let mine = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      s.h.restart().await;
      assert_refused(
          send(&s.h, Method::POST, "linear", &s.token, &[&mine]).await,
          "after a restart",
      )
      .await;
      let again = s.h.session_id(&s.upstream, "linear", &s.token, "up-1").await;
      assert_ne!(again, mine);
      assert_eq!(
          send(&s.h, Method::POST, "linear", &s.token, &[&again]).await.status(),
          StatusCode::OK
      );
  }

  /// An upstream that answers with two session ids: no answer to pass on.
  #[tokio::test]
  async fn an_answer_with_two_session_ids_is_502() {
      let s = setup().await;
      s.upstream.reply(|_, _| {
          Response::builder()
              .header(header::CONTENT_TYPE, "application/json")
              .header("mcp-session-id", "up-1")
              .header("mcp-session-id", "up-2")
              .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#))
              .unwrap()
      });
      let resp = send(&s.h, Method::POST, "linear", &s.token, &[]).await;
      assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
      assert!(resp.headers().get("mcp-session-id").is_none());
      let body: Value = resp.json().await.unwrap();
      assert_eq!(body["code"], "upstream_invalid");
      // One id is wrapped as ever.
      s.upstream.reply(|_, _| {
          let mut resp = json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
          resp.headers_mut().insert("mcp-session-id", "up-1".parse().unwrap());
          resp
      });
      let resp = send(&s.h, Method::POST, "linear", &s.token, &[]).await;
      assert!(resp.headers()["mcp-session-id"].to_str().unwrap().starts_with("up-1."));
  }
  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
      pub client: reqwest::Client,
      task: tokio::task::JoinHandle<()>,
  }

  ```

with:

  ```rust
      pub client: reqwest::Client,
      task: tokio::task::JoinHandle<()>,
      limits: Limits,
  }

  /// The proxy over `world`, on a loopback port of its own.
  async fn serve(world: &World, limits: Limits) -> (SocketAddr, tokio::task::JoinHandle<()>) {
      let egress = Egress::new(Timeouts {
          connect: Duration::from_secs(2),
          request: Duration::from_secs(10),
      })
      .unwrap();
      let gateway = world.gateway();
      let app = router(ProxyState::full(world.proxy_store.clone(), &gateway, egress, limits));
      let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
      let addr = listener.local_addr().unwrap();
      let task = tokio::spawn(async move {
          axum::serve(listener, app).await.unwrap();
      });
      (addr, task)
  }

  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
          let egress = Egress::new(Timeouts {
              connect: Duration::from_secs(2),
              request: Duration::from_secs(10),
          })
          .unwrap();
          let gateway = world.gateway();
          let app = router(ProxyState::full(world.proxy_store.clone(), &gateway, egress, limits));
          let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
          let addr = listener.local_addr().unwrap();
          let task = tokio::spawn(async move {
              axum::serve(listener, app).await.unwrap();
          });
  ```

with:

  ```rust
          let (addr, task) = serve(&world, limits.clone()).await;
  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
              client,
              task,
          }
      }

  ```

with:

  ```rust
              client,
              task,
              limits,
          }
      }

      /// A new proxy on the same world, as after a collector restart: a new
      /// `ProxyState`, on a new port.
      pub async fn restart(&mut self) {
          self.task.abort();
          let (addr, task) = serve(&self.world, self.limits.clone()).await;
          self.addr = addr;
          self.task = task;
      }

  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
          format!("http://{}/mcp/{slug}", self.addr)
      }
  ```

with:

  ```rust
          format!("http://{}/mcp/{slug}", self.addr)
      }

      /// The session id the proxy gives `token` for the upstream session
      /// `upstream_id` (plan 8e decision 13): the upstream answers a `ping`
      /// with it, the proxy wraps it on the way down. It leaves the
      /// upstream's handler set for that ping: a test sets its own after.
      pub async fn session_id(
          &self,
          upstream: &FakeUpstream,
          slug: &str,
          token: &str,
          upstream_id: &'static str,
      ) -> String {
          upstream.reply(move |_, _| {
              let mut resp = json(
                  StatusCode::OK,
                  &serde_json::json!({"jsonrpc": "2.0", "id": 0, "result": {}}),
              );
              resp.headers_mut()
                  .insert("mcp-session-id", HeaderValue::from_static(upstream_id));
              resp
          });
          let resp = self
              .post(
                  slug,
                  token,
                  &serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "ping"}),
              )
              .await;
          assert_eq!(resp.status(), StatusCode::OK);
          let wrapped = resp.headers()["mcp-session-id"].to_str().unwrap().to_owned();
          assert!(wrapped.starts_with(&format!("{upstream_id}.")), "{wrapped}");
          assert_ne!(wrapped, upstream_id);
          wrapped
      }
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test -p hennery-gateway --locked --test session_ids --test proxy
   ```

   It does not compile: `Harness::session_id`, `Limits::with_answer_timeout` and `SessionIds` do not exist.

   ```sh
   git add -A && git commit -m "test(gateway): an upstream session id is the token's own, and a JSON answer has a deadline"
   ```

- [ ] **Step 3: Implement**

In `Cargo.toml`, replace:

  ```toml
  hex = "=0.4.3"
  password-auth = "=1.0.0"
  ```

with:

  ```toml
  hex = "=0.4.3"
  # The MCP gateway binds an upstream's Mcp-Session-Id to its token (plan 8e
  # decision 13): HMAC-SHA256 over sha2 0.10, already in the lock.
  hmac = "=0.12.1"
  password-auth = "=1.0.0"
  ```

In `crates/hennery-gateway/Cargo.toml`, replace:

  ```toml
  hex.workspace = true
  libc = "0.2"
  ```

with:

  ```toml
  hex.workspace = true
  # An upstream's Mcp-Session-Id bound to its token (plan 8e decision 13).
  hmac.workspace = true
  libc = "0.2"
  ```

In `crates/hennery-gateway/Cargo.toml`, replace:

  ```toml
  serde_json.workspace = true
  thiserror.workspace = true
  ```

with:

  ```toml
  serde_json.workspace = true
  sha2.workspace = true
  thiserror.workspace = true
  ```

In `crates/hennery-gateway/src/lib.rs`, replace:

  ```rust
  pub mod session;
  pub mod stdio;
  ```

with:

  ```rust
  pub mod session;
  pub mod session_id;
  pub mod stdio;
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  //!   one type the gateway judged it by (the review's B1).
  //! - **Streaming** (§5.3): a JSON answer is read whole (8 MiB at most) and
  ```

with:

  ```rust
  //!   one type the gateway judged it by (the review's B1).
  //! - **`Mcp-Session-Id`** is bound to the token and the connection (plan 8e
  //!   decision 13, `session_id`): it comes down wrapped and goes up bare;
  //!   one not this token's on this connection, or two, is the 404 above,
  //!   and an answer with two is 502 `upstream_invalid`.
  //! - **Streaming** (§5.3): a JSON answer is read whole (8 MiB at most) and
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  //!   `GET` streams; past either, 503. A request body has 30 s to arrive.
  ```

with:

  ```rust
  //!   `GET` streams; past either, 503. A request body has 30 s to arrive,
  //!   and a JSON answer 60 s from its head to arrive whole (plan 8e decision
  //!   14), else 502 `upstream_unreachable`.
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  use crate::scope::{ClientIdentity, MountPolicy, Principal, ProxyStore, ScopedConnection};
  use crate::store::GatewayStore;
  ```

with:

  ```rust
  use crate::scope::{ClientIdentity, MountPolicy, Principal, ProxyStore, ScopedConnection};
  use crate::session_id::SessionIds;
  use crate::store::GatewayStore;
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  /// `Content-Type` is the gateway's own (the review's O6).
  const FORWARDED_REQUEST_HEADERS: &[&str] = &["accept", "mcp-session-id", "mcp-protocol-version", "last-event-id"];

  /// The response headers passed downstream as they came (gateway spec
  /// §5.2). `Content-Type` is set from what the gateway judged the body to be
  /// (the review's B1), and `Cache-Control` is always `no-store` (plan 8d
  /// decision 19).
  const FORWARDED_RESPONSE_HEADERS: &[&str] = &["mcp-session-id"];
  ```

with:

  ```rust
  /// `Content-Type` is the gateway's own (the review's O6); `Mcp-Session-Id`
  /// goes up unwrapped (`SessionIds`, plan 8e decision 13).
  const FORWARDED_REQUEST_HEADERS: &[&str] = &["accept", "mcp-protocol-version", "last-event-id"];

  /// The one response header passed downstream, wrapped (`SessionIds`, plan
  /// 8e decision 13; gateway spec §5.2). `Content-Type` is set from what the
  /// gateway judged the body to be (the review's B1), and `Cache-Control` is
  /// always `no-store` (plan 8d decision 19).
  const SESSION_ID: &str = "mcp-session-id";
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      pub body_timeout: Duration,
  }
  ```

with:

  ```rust
      pub body_timeout: Duration,
      /// How long a JSON answer may take to arrive whole, from its head (plan
      /// 8e decision 14): it is read whole before any of it goes down, so a
      /// trickling upstream would otherwise hold the request's permit, and
      /// the client wait with no head, for as long as it trickles.
      pub answer_timeout: Duration,
  }
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      pub const BODY_TIMEOUT: Duration = Duration::from_secs(30);

      pub fn new(max_requests: usize, max_streams: usize, head_timeout: Duration, body_timeout: Duration) -> Self {
  ```

with:

  ```rust
      pub const BODY_TIMEOUT: Duration = Duration::from_secs(30);
      pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

      /// `answer_timeout` is `ANSWER_TIMEOUT`; `with_answer_timeout` sets
      /// another.
      pub fn new(max_requests: usize, max_streams: usize, head_timeout: Duration, body_timeout: Duration) -> Self {
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
              body_timeout,
          }
      }
  ```

with:

  ```rust
              body_timeout,
              answer_timeout: Self::ANSWER_TIMEOUT,
          }
      }

      pub fn with_answer_timeout(mut self, answer_timeout: Duration) -> Self {
          self.answer_timeout = answer_timeout;
          self
      }
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      pub revocations: Revocations,
  }
  ```

with:

  ```rust
      pub revocations: Revocations,
      /// What binds an upstream's `Mcp-Session-Id` to its token (plan 8e
      /// decision 13): a key of this process's own.
      pub session_ids: SessionIds,
  }
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
              revocations: gateway.revocations.clone(),
          }
  ```

with:

  ```rust
              revocations: gateway.revocations.clone(),
              session_ids: SessionIds::new(),
          }
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  fn upstream_headers(downstream: &HeaderMap, auth: &UpstreamAuth, json_body: bool) -> HeaderMap {
  ```

with:

  ```rust
  fn upstream_headers(
      downstream: &HeaderMap,
      session: Option<&HeaderValue>,
      auth: &UpstreamAuth,
      json_body: bool,
  ) -> HeaderMap {
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
              out.append(HeaderName::from_static(name), value.clone());
          }
      }
      if !out.contains_key(header::ACCEPT) {
  ```

with:

  ```rust
              out.append(HeaderName::from_static(name), value.clone());
          }
      }
      if let Some(session) = session {
          out.insert(SESSION_ID, session.clone());
      }
      if !out.contains_key(header::ACCEPT) {
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      };
      let permit = if method == Method::GET {
  ```

with:

  ```rust
      };
      // The client's session id, unwrapped (plan 8e decision 13): one that is
      // not this token's on this connection, or more than one, is the same
      // 404 as an unknown token, and nothing goes up.
      let mut sent = headers.get_all(SESSION_ID).iter();
      let session = match (sent.next(), sent.next()) {
          (None, _) => None,
          (Some(id), None) => match state.session_ids.unwrap(token, &connection.id, id) {
              Some(upstream) => Some(upstream),
              None => {
                  tracing::info!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a session id not bound to this token refused");
                  return not_found();
              }
          },
          (Some(_), Some(_)) => {
              tracing::info!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: more than one session id refused");
              return not_found();
          }
      };
      let permit = if method == Method::GET {
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
          *request.headers_mut() = upstream_headers(&headers, auth, body.is_some());
  ```

with:

  ```rust
          *request.headers_mut() = upstream_headers(&headers, session.as_ref(), auth, body.is_some());
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      let mut out = HeaderMap::new();
      for name in FORWARDED_RESPONSE_HEADERS {
          for value in response.headers().get_all(*name) {
              out.append(HeaderName::from_static(name), value.clone());
          }
  ```

with:

  ```rust
      // The upstream's session id goes down wrapped for this token and
      // connection (plan 8e decision 13); two are no answer to pass on.
      let mut answered = response.headers().get_all(SESSION_ID).iter();
      let answered_session = match (answered.next(), answered.next()) {
          (_, Some(_)) => {
              tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: an answer with more than one session id refused");
              return refuse(
                  StatusCode::BAD_GATEWAY,
                  "upstream_invalid",
                  format!("connection {} answered with more than one session id", connection.label),
              );
          }
          (id, None) => id.cloned(),
      };
      let mut out = HeaderMap::new();
      if let Some(id) = &answered_session {
          out.insert(SESSION_ID, state.session_ids.wrap(token, &connection.id, id));
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
      let session_id = response
          .headers()
          .get("mcp-session-id")
          .or_else(|| headers.get("mcp-session-id"))
          .cloned();
  ```

with:

  ```rust
      // The Answerer answers on the upstream's own id, never the wrapped one.
      let session_id = answered_session.or(session);
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
              let read = read_capped(response.bytes_stream(), MAX_FILTERED_BODY).await;
              drop(permits);
              let bytes = match read {
                  Ok(bytes) => bytes,
                  Err(ReadError::TooLarge) => {
  ```

with:

  ```rust
              let read = read_capped(response.bytes_stream(), MAX_FILTERED_BODY);
              let read = tokio::time::timeout(state.limits.answer_timeout, read).await;
              drop(permits);
              let bytes = match read {
                  Ok(Ok(bytes)) => bytes,
                  Err(_) => {
                      tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a JSON answer did not arrive whole in time");
                      return refuse(
                          StatusCode::BAD_GATEWAY,
                          "upstream_unreachable",
                          format!("connection {} did not finish its answer in time", connection.label),
                      );
                  }
                  Ok(Err(ReadError::TooLarge)) => {
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
                  Err(ReadError::Failed) => {
  ```

with:

  ```rust
                  Ok(Err(ReadError::Failed)) => {
  ```

Create `crates/hennery-gateway/src/session_id.rs`:

  ```rust
  //! An upstream's `Mcp-Session-Id`, bound to the token that opened it
  //! (plan 8e decision 13; gateway spec §5.2, §5.5).
  //!
  //! Every token on a connection sends upstream with the connection's one
  //! credential, so the upstream's session id is all that separates two
  //! sessions' upstream state. The proxy keeps no session table: the id the
  //! client sees is `<upstream id>.<tag>`, `tag` the hex of an HMAC-SHA256,
  //! under a key of the process's own, over the bearer token's hash, the
  //! connection's id and the upstream id, each length-prefixed. A request's
  //! id is verified and stripped before it goes up; one that fails is the
  //! same 404 as an unknown token, and nothing is forwarded.
  //!
  //! The key is random per process: after a restart every id is refused, and
  //! the client opens a new session (MCP's streamable HTTP: a 404 on a
  //! request with a session id means "initialize again").

  use crate::tokens::token_hash;
  use axum::http::HeaderValue;
  use hennery_kernel::secret::random_bytes;
  use hmac::{Hmac, Mac};
  use sha2::Sha256;
  use std::sync::Arc;
  use zeroize::Zeroizing;

  type HmacSha256 = Hmac<Sha256>;

  /// The tag's length in hex characters.
  const TAG_HEX: usize = 64;

  /// The process's session-id key. Clones share it.
  #[derive(Clone)]
  pub struct SessionIds {
      key: Arc<Zeroizing<[u8; 32]>>,
  }

  impl SessionIds {
      /// A fresh random key.
      pub fn new() -> Self {
          Self {
              key: Arc::new(Zeroizing::new(random_bytes::<32>())),
          }
      }

      fn mac(&self, token: &str, connection_id: &str, upstream: &[u8]) -> HmacSha256 {
          let mut mac = HmacSha256::new_from_slice(&self.key[..]).expect("HMAC takes a key of any length");
          for part in [token_hash(token).as_bytes(), connection_id.as_bytes(), upstream] {
              mac.update(&(part.len() as u64).to_be_bytes());
              mac.update(part);
          }
          mac
      }

      /// The id the client sees for `upstream`, the id an upstream answered
      /// with on `connection_id` to a request carrying `token`.
      pub fn wrap(&self, token: &str, connection_id: &str, upstream: &HeaderValue) -> HeaderValue {
          let tag = hex::encode(
              self.mac(token, connection_id, upstream.as_bytes())
                  .finalize()
                  .into_bytes(),
          );
          let mut out = upstream.as_bytes().to_vec();
          out.push(b'.');
          out.extend_from_slice(tag.as_bytes());
          HeaderValue::from_bytes(&out).expect("a header value, a dot and hex are a header value")
      }

      /// The upstream id `downstream` wraps, if it was wrapped for `token` on
      /// `connection_id` by this process; else `None`. The tag is compared in
      /// constant time.
      pub fn unwrap(&self, token: &str, connection_id: &str, downstream: &HeaderValue) -> Option<HeaderValue> {
          let bytes = downstream.as_bytes();
          let dot = bytes.iter().rposition(|b| *b == b'.')?;
          let (upstream, tag) = (&bytes[..dot], &bytes[dot + 1..]);
          // Lowercase hex only: one spelling per id.
          if tag.len() != TAG_HEX || !tag.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
              return None;
          }
          let tag = hex::decode(tag).ok()?;
          self.mac(token, connection_id, upstream).verify_slice(&tag).ok()?;
          HeaderValue::from_bytes(upstream).ok()
      }
  }

  impl Default for SessionIds {
      fn default() -> Self {
          Self::new()
      }
  }

  impl std::fmt::Debug for SessionIds {
      fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
          f.write_str("SessionIds(..)")
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      const TOKEN: &str = "hnys_one";

      fn id(s: &str) -> HeaderValue {
          HeaderValue::from_str(s).unwrap()
      }

      #[test]
      fn a_wrapped_id_unwraps_to_the_upstream_id() {
          let ids = SessionIds::new();
          let wrapped = ids.wrap(TOKEN, "c1", &id("up.with.dots-1"));
          assert!(wrapped.to_str().unwrap().starts_with("up.with.dots-1."));
          assert_eq!(ids.unwrap(TOKEN, "c1", &wrapped), Some(id("up.with.dots-1")));
      }

      #[test]
      fn another_token_connection_or_process_is_refused() {
          let ids = SessionIds::new();
          let wrapped = ids.wrap(TOKEN, "c1", &id("up-1"));
          assert_eq!(ids.unwrap("hnys_two", "c1", &wrapped), None, "another token");
          assert_eq!(ids.unwrap(TOKEN, "c2", &wrapped), None, "another connection");
          assert_eq!(SessionIds::new().unwrap(TOKEN, "c1", &wrapped), None, "another process");
          assert_eq!(
              ids.clone().unwrap(TOKEN, "c1", &wrapped),
              Some(id("up-1")),
              "a clone shares the key"
          );
      }

      #[test]
      fn a_bare_forged_or_respelled_id_is_refused() {
          let ids = SessionIds::new();
          let wrapped = ids.wrap(TOKEN, "c1", &id("up-1"));
          let good = wrapped.to_str().unwrap();
          let tag = &good["up-1.".len()..];
          let flipped = if tag.ends_with('0') { '1' } else { '0' };
          for bad in [
              "up-1".to_owned(),
              format!("up-2.{tag}"),
              format!("up-1.{}", tag.to_uppercase()),
              format!("up-1.{}", &tag[..62]),
              format!("up-1.{tag}00"),
              format!("{good}."),
              format!("up-1.{}{flipped}", &tag[..63]),
          ] {
              assert_eq!(ids.unwrap(TOKEN, "c1", &id(&bad)), None, "{bad}");
          }
      }

      /// The parts are length-prefixed: moving bytes between the connection
      /// id and the upstream id changes the tag.
      #[test]
      fn the_parts_cannot_be_shifted() {
          let ids = SessionIds::new();
          let a = ids.wrap(TOKEN, "c1", &id("2up"));
          let b = ids.wrap(TOKEN, "c12", &id("up"));
          // Unprefixed, both would MAC "c12up".
          assert_ne!(&a.as_bytes()[4..], &b.as_bytes()[3..]);
      }
  }
  ```

- [ ] **Step 4: Run the checks**

   Let cargo add `hmac` to the gateway's entry in `Cargo.lock`, then run the five checks:

   ```sh
   nix develop -c cargo check --workspace --all-targets
   ```

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "feat(gateway): bind an upstream's Mcp-Session-Id to its token and connection, and a deadline on JSON answers"
   ```

### Task 6: Differential tests for the stdio set's input and the redaction (lane L16)

**Files:** Modify `crates/hennery-gateway/tests/api.rs`, `tests/support/differential.rs` (`Decoder::get` public), `crates/hennery-sessions/src/redact.rs` (its unit test).
- [ ] **Step 1: Write the tests**

In `crates/hennery-gateway/tests/api.rs`, replace:

  ```rust
  //! Driven through the router in-process.

  ```

with:

  ```rust
  //! Driven through the router in-process.

  // The differential harness (lane L16), for the stdio set's input.
  #[path = "support/differential.rs"]
  mod differential;

  ```

In `crates/hennery-gateway/tests/api.rs`, replace:

  ```rust
          self.hosts.register(id, &enrollment, unix_now()).unwrap();
      }
  ```

with:

  ```rust
          self.hosts.register(id, &enrollment, unix_now()).unwrap();
      }

      /// `PUT` `body` as it is, byte for byte.
      async fn put_raw(&self, session: &str, path: &str, body: &str) -> (StatusCode, Value) {
          let req = Request::builder()
              .method("PUT")
              .uri(path)
              .header("origin", ORIGIN)
              .header("cookie", format!("hennery_session={session}"))
              .header("content-type", "application/json")
              .body(Body::from(body.to_owned()))
              .unwrap();
          let resp = self.app.clone().oneshot(req).await.unwrap();
          let status = resp.status();
          let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
          (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
      }
  ```

In `crates/hennery-gateway/tests/api.rs`, replace:

  ```rust
  }

  #[tokio::test]
  async fn the_stdio_routes_answer_their_codes() {
  ```

with:

  ```rust
  }

  /// One stdio server as `decoder` reads it in a raw `PUT` body: its name,
  /// command, args and env names, or `None` where it finds no string.
  fn server_as_read(decoder: differential::Decoder, server: &differential::Node) -> Option<Value> {
      use differential::Node;
      let strs = |node: Option<&Node>| -> Option<Vec<String>> {
          match node {
              None => Some(Vec::new()),
              Some(Node::Arr(items)) => items.iter().map(|item| decoder.str(Some(item))).collect(),
              Some(_) => None,
          }
      };
      let env = match decoder.get(server, "env") {
          None => Vec::new(),
          Some(Node::Arr(items)) => items
              .iter()
              .map(|item| decoder.str(decoder.get(item, "name")))
              .collect::<Option<Vec<_>>>()?,
          Some(_) => return None,
      };
      Some(json!({
          "name": decoder.str(decoder.get(server, "name"))?,
          "command": decoder.str(decoder.get(server, "command"))?,
          "args": strs(decoder.get(server, "args"))?,
          "env": env,
      }))
  }

  /// Lane L16: the stdio set is a filter on parsed input (names, command,
  /// args, env names, each validated) that a host and an agent read again
  /// later. Each raw body is either refused, changing nothing, or read by
  /// every one of the six decoders exactly as the gateway stored it: a key
  /// twice, a key in another case or with a NUL, a NUL in a value, escapes.
  #[tokio::test]
  async fn every_decoder_reads_an_accepted_stdio_set_as_it_was_stored() {
      let api = Api::new();
      api.host("host-a", 1);
      let hat = api.hat();
      let path = format!("/api/mcp/stdio-servers?host_id=host-a&hat_id={hat}");
      let fresh = api.session(0);
      let vectors: &[(&str, bool)] = &[
          (
              r#"{"servers":[{"name":"files","command":"files-mcp","args":["--root","/srv"],"env":[{"name":"K","value":"v"}]}]}"#,
              true,
          ),
          (
              r#"{"servers":[{"name":"\u0066iles","command":"files\u002dmcp"}]}"#,
              true,
          ),
          (
              r#"{"servers":[{"name":"files","command":"safe","command":"evil"}]}"#,
              false,
          ),
          (r#"{"servers":[],"servers":[{"name":"files","command":"evil"}]}"#, false),
          (
              r#"{"servers":[{"name":"files","command":"safe","Command":"evil"}]}"#,
              false,
          ),
          (
              r#"{"servers":[{"name":"files","command":"safe","command\u0000x":"evil"}]}"#,
              false,
          ),
          (r#"{"ſervers":[{"name":"files","command":"evil"}],"servers":[]}"#, false),
          (r#"{"servers":[{"name":"files","command":"safe\u0000evil"}]}"#, false),
          (r#"{"servers":[{"name":"files\u0000x","command":"safe"}]}"#, false),
          (
              r#"{"servers":[{"name":"files","command":"safe","args":["a\u0000b"]}]}"#,
              false,
          ),
          (
              r#"{"servers":[{"name":"files","command":"safe","env":[{"name":"K\u0000X","value":"v"}]}]}"#,
              false,
          ),
          (
              r#"{"servers":[{"name":"files","command":"safe","env":[{"name":"K","name":"X","value":"v"}]}]}"#,
              false,
          ),
          (
              r#"{"servers":[{"name":"files","command":"safe","env":[{"name":"K","Name":"X","value":"v"}]}]}"#,
              false,
          ),
          (
              r#"{"servers":[{"name":"files","command":"safe","env":[{"name":"K","value":"a\u0000b"}]}]}"#,
              false,
          ),
      ];
      for &(raw, accepted) in vectors {
          // Each from an empty set: a refused one leaves it empty.
          let (status, _) = api.send(&fresh, "PUT", &path, Some(&json!({"servers": []}))).await;
          assert_eq!(status, StatusCode::OK);
          let (status, answer) = api.put_raw(&fresh, &path, raw).await;
          let (_, stored) = api.send(&fresh, "GET", &path, None).await;
          if !accepted {
              assert!(status.is_client_error(), "{raw}: {status} {answer}");
              assert_eq!(stored["servers"], json!([]), "{raw}");
              continue;
          }
          assert_eq!(status, StatusCode::OK, "{raw}: {answer}");
          let as_stored: Vec<Value> = stored["servers"]
              .as_array()
              .unwrap()
              .iter()
              .map(|s| {
                  let env: Vec<&Value> = s["env"].as_array().unwrap().iter().map(|e| &e["name"]).collect();
                  json!({"name": s["name"], "command": s["command"], "args": s["args"], "env": env})
              })
              .collect();
          let node: differential::Node = serde_json::from_str(raw).unwrap();
          for &decoder in differential::DECODERS {
              let Some(differential::Node::Arr(servers)) = decoder.get(&node, "servers") else {
                  panic!("{decoder:?} finds no servers in {raw}");
              };
              let as_read: Vec<Value> = servers
                  .iter()
                  .map(|server| server_as_read(decoder, server).unwrap_or(Value::Null))
                  .collect();
              assert_eq!(as_read, as_stored, "{decoder:?} reads {raw} otherwise");
          }
      }
  }

  #[tokio::test]
  async fn the_stdio_routes_answer_their_codes() {
  ```

In `crates/hennery-gateway/tests/support/differential.rs`, replace:

  ```rust
      fn get<'a>(self, node: &'a Node, name: &str) -> Option<&'a Node> {
  ```

with:

  ```rust
      pub fn get<'a>(self, node: &'a Node, name: &str) -> Option<&'a Node> {
  ```

In `crates/hennery-sessions/src/redact.rs`, replace:

  ```rust
  #[cfg(test)]
  mod tests {
  ```

with:

  ```rust
  // The gateway's differential harness (lane L16): its decoders read what
  // this module stores and forwards.
  #[cfg(test)]
  #[path = "../../hennery-gateway/tests/support/differential.rs"]
  mod differential;

  #[cfg(test)]
  mod tests {
      use super::differential::{DECODERS, Decoder, Node};
  ```

In `crates/hennery-sessions/src/redact.rs`, replace:

  ```rust
      }

      #[test]
      fn every_string_of_a_value_is_redacted_keys_too() {
  ```

with:

  ```rust
      }

      /// A token-shaped run, judged apart from `text`: the prefix in any
      /// case, then 8 or more hexadecimal digits.
      fn has_token(s: &str) -> bool {
          let lower = s.to_ascii_lowercase();
          lower.match_indices(SESSION_TOKEN_PREFIX).any(|(at, prefix)| {
              lower[at + prefix.len()..]
                  .bytes()
                  .take_while(u8::is_ascii_hexdigit)
                  .count()
                  >= MIN_DIGITS
          })
      }

      /// Every string and key of `node` as `decoder` hands it on.
      fn strings(decoder: Decoder, node: &Node, out: &mut Vec<String>) {
          match node {
              Node::Str(_) => out.extend(decoder.str(Some(node))),
              Node::Arr(items) => items.iter().for_each(|item| strings(decoder, item, out)),
              Node::Obj(entries) => {
                  for (key, value) in entries {
                      out.push(decoder.key(key));
                      out.push(key.clone());
                      strings(decoder, value, out);
                  }
              }
              Node::Null | Node::Bool | Node::Num(_) => {}
          }
      }

      /// Lane L16: a payload as a host may send it, read as serde reads a
      /// frame, redacted, and written back as hennery stores and forwards
      /// it; then read by each of the six decoders, which find no
      /// token-shaped run in any string or key. A token in a key repeated
      /// (the first or the last copy), spelled with escapes, upper-cased, or
      /// in a key a decoder folds, is redacted or gone.
      #[test]
      fn no_decoder_reads_a_token_in_what_is_stored() {
          let t = token();
          let upper = t.to_uppercase();
          let escaped = t.replace('_', "\\u005f");
          let vectors = [
              format!(r#"{{"text":"{t}"}}"#),
              format!(r#"{{"text":"{t}","text":"clean"}}"#),
              format!(r#"{{"text":"clean","text":"{t}"}}"#),
              format!(r#"{{"text":"{escaped}"}}"#),
              format!(r#"{{"Text":"{upper}","text":"clean"}}"#),
              format!(r#"{{"{t}":1,"{t}":2}}"#),
              format!(r#"{{"te\u0000xt":"{t}"}}"#),
              format!(r#"{{"\u017fession":"{t}","session":"x"}}"#),
              format!(r#"[{{"a":["x","{t}"]}},"{t} and {t}"]"#),
          ];
          for raw in vectors {
              let mut payload: Value = serde_json::from_str(&raw).unwrap();
              value(&mut payload);
              let stored = serde_json::to_vec(&payload).unwrap();
              assert!(!has_token(&String::from_utf8_lossy(&stored)), "{raw}");
              let node: Node = serde_json::from_slice(&stored).unwrap();
              for &decoder in DECODERS {
                  let mut seen = Vec::new();
                  strings(decoder, &node, &mut seen);
                  assert!(!seen.iter().any(|s| has_token(s)), "{decoder:?} reads a token in {raw}");
              }
          }
      }

      #[test]
      fn every_string_of_a_value_is_redacted_keys_too() {
  ```

- [ ] **Step 2: Run them**

   ```sh
   nix develop -c cargo test -p hennery-gateway --locked --test api every_decoder
   nix develop -c cargo test -p hennery-sessions --locked --lib no_decoder
   ```

   They pass at once: they pin what Tasks 3 and 4 built against the six decoders of plan "gateway differential" (lane L16); probes 102–109 are theirs. Then run the five checks.

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```
- [ ] **Step 3: Commit**

   ```sh
   git add -A && git commit -m "test: differential tests for the stdio set's input and the redaction (lane L16)"
   ```

### Task 7: A hat's purge runs the gateway's part first

**Files:** Modify `crates/hennery-sessions/src/hats.rs`, `src/lib.rs`, `tests/session_mcp.rs` (a doc comment); `crates/hennery-testkit/tests/purge.rs`.
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-sessions/tests/session_mcp.rs`, replace:

  ```rust
  /// the sessions' part, then the gateway's, so the hat's row can go.
  ```

with:

  ```rust
  /// the gateway's part, then the sessions', so the hat's row can go.
  ```

In `crates/hennery-testkit/tests/purge.rs`, replace:

  ```rust
          tokio::time::sleep(Duration::from_millis(100)).await;
      }
  }
  ```

with:

  ```rust
          tokio::time::sleep(Duration::from_millis(100)).await;
      }
  }

  /// The collector's gateway on `c`'s database, given to its sessions (plan
  /// 8e, as `hennery`'s `gateway()` does).
  fn gateway(c: &Collector) -> hennery_gateway::api::GatewayState {
      let keys = hennery_gateway::key::KeySource::from_vars(c._dir.path(), Some("07".repeat(32).into()), None).unwrap();
      let gateway = hennery_gateway::open(&c.db, &keys, c.state.operator.clone()).unwrap();
      c.state
          .store
          .set_session_mcp(std::sync::Arc::new(hennery_gateway::session::GatewayMcp::new(&gateway)));
      gateway
  }

  /// A connection labelled `label`, a stdio server on `HOST` running
  /// `label`, and a session token, in `hat`.
  fn gateway_rows(c: &Collector, gateway: &hennery_gateway::api::GatewayState, hat: &str, session: &str, label: &str) {
      let slug = format!("linear-{}", &hat[hat.len() - 6..]);
      let new = hennery_gateway::model::NewConnection {
          slug: slug.clone(),
          label: label.into(),
          url: "https://mcp.linear.example/mcp".into(),
          hat_id: hat.into(),
          cred_kind: hennery_gateway::model::CredKind::None,
          static_header: None,
          static_prefix: None,
          tool_allowlist: None,
          internal_network: false,
      };
      assert!(matches!(
          gateway.store.create(&new, NOW).unwrap(),
          hennery_gateway::model::Change::Done(_)
      ));
      let stdio = hennery_gateway::stdio::StdioInput {
          name: format!("files-{}", &hat[hat.len() - 6..]),
          command: label.into(),
          args: vec![],
          env: vec![],
      };
      assert!(matches!(
          gateway
              .store
              .replace_stdio_set(HOST, hat, &[stdio], &gateway.key, NOW)
              .unwrap(),
          hennery_gateway::stdio::StdioChange::Done(_)
      ));
      let mut conn = Connection::open(&c.db).unwrap();
      let tx = conn.transaction().unwrap();
      hennery_gateway::tokens::mint_in(&tx, gateway.store.owner_id(), session, HOST, hat, NOW).unwrap();
      tx.commit().unwrap();
  }

  /// How many of the gateway's rows `hat` has: connections, stdio servers,
  /// session tokens.
  fn gateway_rows_of(c: &Collector, hat: &str) -> i64 {
      let conn = Connection::open(&c.db).unwrap();
      ["gw_connections", "gw_stdio_servers", "gw_session_tokens"]
          .iter()
          .map(|table| {
              conn.query_row(&format!("SELECT count(*) FROM {table} WHERE hat_id = ?1"), [hat], |r| {
                  r.get::<_, i64>(0)
              })
              .unwrap()
          })
          .sum()
  }

  /// Plan 9c's hand-off to plan 8 (lane L6, A15): the purge route runs the
  /// gateway's part at its reserved place, before the sessions': the hat's
  /// connections, stdio servers and session tokens go, another hat's stay,
  /// and the hat row can go after them; nothing of the gateway's rows is
  /// left in the database's files (A8). Run again, the part deletes nothing
  /// and succeeds.
  #[tokio::test]
  async fn a_purge_takes_the_hats_gateway_rows_and_leaves_another_hats() {
      let (c, acme, other) = Collector::start().await;
      let gateway = gateway(&c);
      c.parked("a1", HOST, &acme);
      c.parked("o1", HOST, &other);
      let marker = "okapi-ledger-7d1c";
      gateway_rows(&c, &gateway, &acme, "a1", marker);
      gateway_rows(&c, &gateway, &other, "o1", "Linear");
      assert_eq!(gateway_rows_of(&c, &acme), 3);
      let result = c.purged(&acme).await;
      assert_eq!(result.sessions, 1);
      assert_eq!(gateway_rows_of(&c, &acme), 0);
      assert_eq!(gateway_rows_of(&c, &other), 3);
      let dir = c.db.parent().unwrap();
      for file in ["hennery.db", "hennery.db-wal"] {
          let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
          assert!(
              !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
              "{file} still holds the hat's gateway rows"
          );
      }
      assert_eq!(c.state.hosts.hat(&acme).unwrap(), None);
      c.state.on_hat_purged(&acme).unwrap();
      assert_eq!(gateway_rows_of(&c, &other), 3);
  }

  /// The gateway's part only, failing or recording whether the purge's
  /// checkpoint was owed when it ran: everything else is no gateway's.
  struct FailingPurge;

  struct RecordingPurge {
      owed_marker: PathBuf,
      owed: std::sync::Arc<std::sync::Mutex<Vec<bool>>>,
  }

  impl hennery_gateway::session::SessionMcp for FailingPurge {
      fn servers_in(
          &self,
          tx: &rusqlite::Transaction<'_>,
          session: hennery_gateway::session::SessionRef<'_>,
          mode: hennery_proto::rest::McpSessionDeliveryMode,
      ) -> anyhow::Result<hennery_gateway::session::Delivered> {
          hennery_gateway::session::NoSessionMcp.servers_in(tx, session, mode)
      }
      fn revoke_in(&self, tx: &rusqlite::Transaction<'_>, id: &str) -> anyhow::Result<hennery_gateway::revocation::Cut> {
          hennery_gateway::session::NoSessionMcp.revoke_in(tx, id)
      }
      fn revoke_host_in(
          &self,
          tx: &rusqlite::Transaction<'_>,
          id: &str,
      ) -> anyhow::Result<hennery_gateway::revocation::Cut> {
          hennery_gateway::session::NoSessionMcp.revoke_host_in(tx, id)
      }
      fn cut(&self, _: hennery_gateway::revocation::Cut) {}
      fn purge_hat(&self, _: &str) -> anyhow::Result<()> {
          anyhow::bail!("the gateway's part failed")
      }
  }

  impl hennery_gateway::session::SessionMcp for RecordingPurge {
      fn servers_in(
          &self,
          tx: &rusqlite::Transaction<'_>,
          session: hennery_gateway::session::SessionRef<'_>,
          mode: hennery_proto::rest::McpSessionDeliveryMode,
      ) -> anyhow::Result<hennery_gateway::session::Delivered> {
          hennery_gateway::session::NoSessionMcp.servers_in(tx, session, mode)
      }
      fn revoke_in(&self, tx: &rusqlite::Transaction<'_>, id: &str) -> anyhow::Result<hennery_gateway::revocation::Cut> {
          hennery_gateway::session::NoSessionMcp.revoke_in(tx, id)
      }
      fn revoke_host_in(
          &self,
          tx: &rusqlite::Transaction<'_>,
          id: &str,
      ) -> anyhow::Result<hennery_gateway::revocation::Cut> {
          hennery_gateway::session::NoSessionMcp.revoke_host_in(tx, id)
      }
      fn cut(&self, _: hennery_gateway::revocation::Cut) {}
      fn purge_hat(&self, _: &str) -> anyhow::Result<()> {
          self.owed.lock().unwrap().push(self.owed_marker.exists());
          Ok(())
      }
  }

  /// A15's order: the gateway's part runs first, so when it fails the purge
  /// stops before deleting any session, on the route and in the hook alike;
  /// the hat stays frozen, and a purge again completes it.
  #[tokio::test]
  async fn a_failed_gateway_part_stops_a_purge_before_its_sessions() {
      let (c, acme, _) = Collector::start().await;
      c.parked("a1", HOST, &acme);
      c.state.store.set_session_mcp(std::sync::Arc::new(FailingPurge));
      let (status, code, _) = code_of(c.purge(&acme).await).await;
      assert_eq!((status, code.as_str()), (500, "internal"));
      assert_eq!(c.lifecycle("a1").as_deref(), Some("parked"));
      assert!(c.state.hosts.hat(&acme).unwrap().unwrap().purging);
      assert!(c.state.on_hat_purged(&acme).is_err());
      assert_eq!(c.lifecycle("a1").as_deref(), Some("parked"));
      c.state
          .store
          .set_session_mcp(std::sync::Arc::new(hennery_gateway::session::NoSessionMcp));
      assert_eq!(c.purged(&acme).await.sessions, 1);
  }

  /// A8 for the gateway's part: its deletes are the purge's first, so the
  /// purge's one checkpoint is owed (durably, `<db>-checkpoint-owed`) before
  /// they run, on the route and in the hook alike; a crash in them leaves it
  /// owed.
  #[tokio::test]
  async fn the_purges_checkpoint_is_owed_before_the_gateways_part() {
      let (c, acme, other) = Collector::start().await;
      c.parked("a1", HOST, &acme);
      c.parked("o1", HOST, &other);
      let mut owed_marker = c.db.clone().into_os_string();
      owed_marker.push("-checkpoint-owed");
      let owed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
      c.state.store.set_session_mcp(std::sync::Arc::new(RecordingPurge {
          owed_marker: owed_marker.into(),
          owed: owed.clone(),
      }));
      assert_eq!(c.purged(&acme).await.sessions, 1);
      assert!(matches!(
          c.state.hosts.begin_purge(&other, NOW).unwrap(),
          PurgeStart::Frozen { .. }
      ));
      c.state.on_hat_purged(&other).unwrap();
      assert_eq!(*owed.lock().unwrap(), [true, true], "route, then hook");
  }
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test -p hennery-testkit --locked --test purge
   ```

   `a_purge_takes_the_hats_gateway_rows_and_leaves_another_hats`, `a_failed_gateway_part_stops_a_purge_before_its_sessions` and `the_purges_checkpoint_is_owed_before_the_gateways_part` fail: the route does not run the gateway's part, and the hook runs it after the sessions'.

   ```sh
   git add -A && git commit -m "test(sessions): a hat's purge runs the gateway's part first, its checkpoint owed before it"
   ```

- [ ] **Step 3: Implement**

In `crates/hennery-sessions/src/hats.rs`, replace:

  ```rust
      // plan 8: the gateway's `on_hat_purged` runs here, before the sessions'
      // (A15), so a hat stuck frozen cannot reach MCP meanwhile.
      // One checkpoint for the whole purge, on every way out from here (plan
      // 9a A8): owed before the first delete, so a crash leaves it owed.
      state.store.owe_checkpoint();
      let purged = purge_sessions(&state, &id).and_then(|purged| {
          // `false`: a purge alongside got there first; it is done either way.
          state.hosts.finish_purge(&id)?;
          Ok(purged)
      });
  ```

with:

  ```rust
      // One checkpoint for the whole purge, on every way out from here (plan
      // 9a A8): owed before the first delete, so a crash leaves it owed.
      state.store.owe_checkpoint();
      // The gateway's part first (plan 8e, lane L6): the hat's connections,
      // stdio servers and session tokens, their open streams cut. Before the
      // sessions' (A15), so a hat stuck frozen cannot reach MCP meanwhile;
      // idempotent, so a purge posted again runs it again.
      let purged = state
          .store
          .purge_gateway_hat(&id)
          .and_then(|()| purge_sessions(&state, &id))
          .and_then(|purged| {
              // `false`: a purge alongside got there first; it is done either way.
              state.hosts.finish_purge(&id)?;
              Ok(purged)
          });
  ```

In `crates/hennery-sessions/src/lib.rs`, replace:

  ```rust
      /// The session module's part of a hat's purge (plan 9c decision 10d):
      /// `hats::purge_sessions`, which the purge route calls itself for what
      /// it deleted, then the gateway's part (lane L6, plan 8e: its tokens,
      /// stdio servers and connections, with their open streams cut), and
      /// the one checkpoint the deletes owe. Each is idempotent: a purge that
      /// stopped runs both again.
      fn on_hat_purged(&self, hat_id: &str) -> anyhow::Result<()> {
          self.store.owe_checkpoint();
          let purged = hats::purge_sessions(self, hat_id)
              .map(drop)
              .and_then(|()| self.store.purge_gateway_hat(hat_id));
  ```

with:

  ```rust
      /// A hat's purge past its freeze, as the purge route runs it: the
      /// gateway's part first (lane L6, A15; plan 8e: its tokens, stdio
      /// servers and connections, with their open streams cut), then the
      /// session module's (plan 9c decision 10d, `hats::purge_sessions`),
      /// and the one checkpoint the deletes owe. Each is idempotent: a purge
      /// that stopped runs both again.
      fn on_hat_purged(&self, hat_id: &str) -> anyhow::Result<()> {
          self.store.owe_checkpoint();
          let purged = self
              .store
              .purge_gateway_hat(hat_id)
              .and_then(|()| hats::purge_sessions(self, hat_id).map(drop));
  ```

- [ ] **Step 4: Run the checks**

   Run the five checks:

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "feat(sessions): a hat's purge runs the gateway's part first, its checkpoint owed before it (lane L6)"
   ```

### Task 8: The security review's amendments

**Files:** Create `crates/hennery-sessions/tests/cut_after_commit.rs`; modify `crates/hennery-gateway/src/proxy.rs`, `src/revocation.rs`, `tests/proxy.rs`, `tests/revocation.rs`, `tests/differential.rs`, `tests/support/upstream.rs`; `crates/hennery-sessions/src/store.rs`, `src/hosts.rs`; `crates/hennery-testkit/tests/session_gateway.rs`.
- [ ] **Step 1: Write the failing tests**

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
      let s = setup().await;
      let replayed = "id: 9\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"search\"},{\"name\":\"delete\"}]}}\n\n";
  ```

with:

  ```rust
      let s = setup().await;
      // A replay's cursor goes up only with a bound session id (plan 8e).
      let session =
          s.h.session_id(&s.upstream, "linear", &s.token, "upstream-session-1")
              .await;
      let replayed = "id: 9\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[{\"name\":\"search\"},{\"name\":\"delete\"}]}}\n\n";
  ```

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
                  .bearer_auth(&s.token)
                  .header(header::ACCEPT, "text/event-stream");
  ```

with:

  ```rust
                  .bearer_auth(&s.token)
                  .header("mcp-session-id", &session)
                  .header(header::ACCEPT, "text/event-stream");
  ```

In `crates/hennery-gateway/tests/differential.rs`, replace:

  ```rust
      assert_eq!(seen[1].header("last-event-id"), Some("8"));
  ```

with:

  ```rust
      assert_eq!(seen[2].header("last-event-id"), Some("8"));
  ```

In `crates/hennery-gateway/tests/proxy.rs`, replace:

  ```rust

  /// Plan 8e decision 14: a JSON answer, read whole before any of it goes
  ```

with:

  ```rust

  /// A replay's cursor (`Last-Event-ID`) goes up only beside a session id
  /// bound to this token (plan 8e, the security review's finding 4): an
  /// upstream that replays by event id alone cannot be asked for another
  /// session's stream by a token that has none of its own.
  #[tokio::test]
  async fn a_replay_cursor_goes_up_only_with_a_bound_session_id() {
      let s = setup(CredKind::None, None).await;
      let session =
          s.h.session_id(&s.upstream, "linear", &s.token, "upstream-session-1")
              .await;
      s.upstream.reply(|_, _| sse(&[]));
      for with_session in [false, true] {
          let mut get =
              s.h.client
                  .get(s.h.url("linear"))
                  .bearer_auth(&s.token)
                  .header(header::ACCEPT, "text/event-stream")
                  .header("last-event-id", "41");
          if with_session {
              get = get.header("mcp-session-id", &session);
          }
          assert_eq!(get.send().await.unwrap().status(), StatusCode::OK);
      }
      let seen = s.upstream.seen();
      assert_eq!(seen.len(), 3, "the session's ping, then the two streams");
      assert_eq!(
          seen[1].header("last-event-id"),
          None,
          "a cursor without a session id went up"
      );
      assert_eq!(seen[2].header("mcp-session-id"), Some("upstream-session-1"));
      assert_eq!(seen[2].header("last-event-id"), Some("41"));
  }

  /// Plan 8e decision 14: a JSON answer, read whole before any of it goes
  ```

In `crates/hennery-gateway/tests/revocation.rs`, replace:

  ```rust
  use hennery_gateway::session::{GatewayMcp, SessionMcp};
  ```

with:

  ```rust
  use hennery_gateway::scope::{ClientIdentity, Principal};
  use hennery_gateway::session::{GatewayMcp, SessionMcp};
  use std::sync::{Arc, Mutex};
  ```

In `crates/hennery-gateway/tests/revocation.rs`, replace:

  ```rust

  /// The watch goes when the request does: nothing is kept per token once
  ```

with:

  ```rust

  /// A resolve that read the token live, then saw a revoke commit and cut
  /// before it returned, as a revoke in flight can: it answers what it read,
  /// once and again.
  struct RevokedDuringResolve {
      inner: Arc<dyn ClientIdentity>,
      revoke: Box<dyn Fn() + Send + Sync>,
      read: Mutex<Option<Principal>>,
  }

  impl ClientIdentity for RevokedDuringResolve {
      fn resolve(&self, token: &str, now: i64) -> anyhow::Result<Option<Principal>> {
          let mut read = self.read.lock().unwrap();
          if read.is_none() {
              *read = self.inner.resolve(token, now)?;
              (self.revoke)();
          }
          Ok(read.clone())
      }
  }

  /// The race decision 12's order closes (the security review's finding 1):
  /// a revoke that commits and cuts while the proxy resolves the token finds
  /// its watch already registered, so the request ends with the same 404,
  /// however live the token read.
  #[tokio::test]
  async fn a_revoke_committed_during_the_resolve_ends_the_request() {
      let upstream = FakeUpstream::start().await;
      let mut h = Harness::new().await;
      mounted(&h, &upstream);
      let token = h.mint("s1", "host-a", &h.hat());
      let mcp = GatewayMcp::new(&h.gateway());
      let db = h.db.clone();
      h.restart_with(move |state| {
          state.identity = Arc::new(RevokedDuringResolve {
              inner: state.identity.clone(),
              revoke: Box::new(move || {
                  let mut conn = hennery_kernel::db::open(&db).unwrap();
                  let tx = conn.transaction().unwrap();
                  let cut = mcp.revoke_in(&tx, "s1").unwrap();
                  tx.commit().unwrap();
                  mcp.cut(cut);
              }),
              read: Mutex::new(None),
          });
      })
      .await;
      let response = tokio::time::timeout(BOUND, h.post("linear", &token, &support::upstream::list(1)))
          .await
          .unwrap();
      assert_eq!(response.status(), StatusCode::NOT_FOUND);
  }

  /// Decision 12's "the JSON read included" (the security review's finding
  /// 3): a revoke while a JSON answer is still being read whole ends the
  /// request with the same 404, before the answer's deadline.
  #[tokio::test]
  async fn a_revoke_while_a_json_answer_is_read_ends_the_request() {
      let upstream = FakeUpstream::start().await;
      let h = Harness::with_limits(
          Limits::new(8, 8, Duration::from_secs(60), Duration::from_secs(60))
              .with_answer_timeout(Duration::from_secs(60)),
      )
      .await;
      mounted(&h, &upstream);
      // A JSON answer whose body never ends.
      upstream.reply(|_, hold| {
          let hold = hold.clone();
          let body = futures::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"{")) }).chain(
              futures::stream::unfold(hold, |mut hold| async move {
                  while hold.changed().await.is_ok() {}
                  None
              }),
          );
          Response::builder()
              .header(header::CONTENT_TYPE, "application/json")
              .body(Body::from_stream(body))
              .unwrap()
      });
      let token = h.mint("s1", "host-a", &h.hat());
      let request = {
          let client = h.client.clone();
          let url = h.url("linear");
          let token = token.clone();
          tokio::spawn(async move {
              client
                  .post(url)
                  .bearer_auth(token)
                  .header(header::CONTENT_TYPE, "application/json")
                  .body(support::upstream::list(1).to_string())
                  .send()
                  .await
          })
      };
      // Positive signal: the upstream got the request, so its answer is
      // being read.
      let deadline = tokio::time::Instant::now() + BOUND;
      while upstream.seen().is_empty() {
          assert!(tokio::time::Instant::now() < deadline, "the request never went up");
          tokio::time::sleep(Duration::from_millis(10)).await;
      }
      revoke(&h, "s1");
      let response = tokio::time::timeout(BOUND, request).await.unwrap().unwrap().unwrap();
      assert_eq!(response.status(), StatusCode::NOT_FOUND);
  }

  /// The watch goes when the request does: nothing is kept per token once
  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
  async fn serve(world: &World, limits: Limits) -> (SocketAddr, tokio::task::JoinHandle<()>) {
      let egress = Egress::new(Timeouts {
  ```

with:

  ```rust
  async fn serve(world: &World, limits: Limits) -> (SocketAddr, tokio::task::JoinHandle<()>) {
      serve_edited(world, limits, |_| {}).await
  }

  /// As `serve`, with the proxy's state changed by `edit` first.
  async fn serve_edited(
      world: &World,
      limits: Limits,
      edit: impl FnOnce(&mut ProxyState),
  ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
      let egress = Egress::new(Timeouts {
  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
      let app = router(ProxyState::full(world.proxy_store.clone(), &gateway, egress, limits));
  ```

with:

  ```rust
      let mut state = ProxyState::full(world.proxy_store.clone(), &gateway, egress, limits);
      edit(&mut state);
      let app = router(state);
  ```

In `crates/hennery-gateway/tests/support/upstream.rs`, replace:

  ```rust
          let (addr, task) = serve(&self.world, self.limits.clone()).await;
          self.addr = addr;
  ```

with:

  ```rust
          let (addr, task) = serve(&self.world, self.limits.clone()).await;
          self.addr = addr;
          self.task = task;
      }

      /// A new proxy on the same world, its state changed by `edit` (a
      /// test's own `ClientIdentity`), on a new port.
      pub async fn restart_with(&mut self, edit: impl FnOnce(&mut ProxyState)) {
          self.task.abort();
          let (addr, task) = serve_edited(&self.world, self.limits.clone(), edit).await;
          self.addr = addr;
  ```

Create `crates/hennery-sessions/tests/cut_after_commit.rs`:

  ```rust
  //! Plan 8e decision 12, held by the source (the security review's finding
  //! 1): the sessions module ends what is open on a revoked token only after
  //! the transaction that revoked it has committed. Every cut goes through
  //! `store.rs`'s `commit_then_cut`, which commits first, so no site can cut
  //! before its commit, or forget to commit first, without failing here.

  use std::path::Path;

  /// Every Rust source of the crate, with its path, nested modules included.
  fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
      for entry in std::fs::read_dir(dir).unwrap() {
          let path = entry.unwrap().path();
          if path.is_dir() {
              sources(&path, out);
          } else if path.extension().is_some_and(|e| e == "rs") {
              out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
          }
      }
  }

  #[test]
  fn every_cut_goes_through_commit_then_cut() {
      let mut files = Vec::new();
      sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
      assert!(files.len() > 10, "the crate's sources were not found");
      let mut cuts = Vec::new();
      for (path, text) in &files {
          for (n, line) in text.lines().enumerate() {
              if line.contains(".cut(") {
                  cuts.push(format!("{path}:{}: {}", n + 1, line.trim()));
              }
          }
      }
      assert_eq!(cuts.len(), 1, "a cut outside `commit_then_cut`: {cuts:#?}");
      let store = &files.iter().find(|(path, _)| path.ends_with("store.rs")).unwrap().1;
      let helper = store
          .split("fn commit_then_cut(")
          .nth(1)
          .expect("store.rs has no `commit_then_cut`");
      let body = &helper[..helper.find("\n}\n").unwrap()];
      let (commit, cut) = (body.find("tx.commit()?;").unwrap(), body.find(".cut(").unwrap());
      assert!(commit < cut, "`commit_then_cut` cuts before it commits");
      assert!(cuts[0].contains("mcp.cut(cut)"), "{cuts:?}");
  }
  ```

In `crates/hennery-testkit/tests/session_gateway.rs`, replace:

  ```rust
      );
  }
  ```

with:

  ```rust
      );
  }

  /// The security review's finding 2: a host revoke cuts the streams open on
  /// the host's tokens at once, not only once its connection has closed. The
  /// host here holds a connection that never closes, so the route waits its
  /// whole bound (10 s) for it; the stream ends well before that.
  #[tokio::test]
  async fn a_host_revoke_cuts_its_streams_without_waiting_for_its_connection() {
      let dir = tempfile::tempdir().unwrap();
      let (upstream, _) = upstream().await;
      let c = Collector::start(dir.path(), upstream).await;
      // A connection that never ends: the revoke's wait for it runs out.
      let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
      let _held = c
          .state
          .hub
          .register(HOST, tx, Default::default(), Default::default())
          .expect("registered");
      let token = {
          let mut conn = rusqlite::Connection::open(&c.db).unwrap();
          let tx = conn.transaction().unwrap();
          let token = hennery_gateway::tokens::mint_in(
              &tx,
              c.gateway.store.owner_id(),
              "s1",
              HOST,
              &c.hat,
              hennery_kernel::secret::unix_now(),
          )
          .unwrap();
          tx.commit().unwrap();
          token.expose().to_string()
      };
      let response = c
          .proxy_client
          .get(c.url("/mcp/linear"))
          .bearer_auth(&token)
          .header(header::ACCEPT, "text/event-stream")
          .send()
          .await
          .unwrap();
      assert_eq!(response.status(), StatusCode::OK);
      let mut stream = response.bytes_stream();
      let first = tokio::time::timeout(BOUND, stream.next()).await.unwrap();
      assert!(matches!(first, Some(Ok(_))), "the first event");
      let revoke = {
          let client = c.client.clone();
          let url = c.url(&format!("/api/hosts/{HOST}"));
          tokio::spawn(async move { client.delete(url).send().await.unwrap().status() })
      };
      let started = std::time::Instant::now();
      // The stream ends (broken, or closed) without another event.
      let next = tokio::time::timeout(BOUND, stream.next()).await.unwrap();
      assert!(!matches!(next, Some(Ok(_))), "another event came: {next:?}");
      assert!(
          started.elapsed() < Duration::from_secs(5),
          "the stream was cut only after {:?}, with the wait for the connection",
          started.elapsed()
      );
      // Revoked too, not only cut (the re-confirmation's note 1): while the
      // route still waits, the token's row is revoked, and the token opens
      // nothing new (that, the host's own revoke already refuses).
      assert!(
          !revoke.is_finished(),
          "the route no longer waits: the test proves nothing"
      );
      let live: i64 = rusqlite::Connection::open(&c.db)
          .unwrap()
          .query_row(
              "SELECT count(*) FROM gw_session_tokens WHERE session_id = 's1' AND revoked_at IS NULL",
              [],
              |r| r.get(0),
          )
          .unwrap();
      assert_eq!(live, 0, "the token's row is still live while the route waits");
      assert_eq!(c.call("linear", &token).await, 404);
      assert_eq!(
          tokio::time::timeout(BOUND, revoke).await.unwrap().unwrap(),
          StatusCode::OK
      );
  }
  ```

- [ ] **Step 2: Run them and see them fail, then commit them**

   ```sh
   nix develop -c cargo test --locked -p hennery-gateway --test revocation --test proxy --test differential && nix develop -c cargo test --locked -p hennery-sessions --test cut_after_commit && nix develop -c cargo test --locked -p hennery-testkit --test session_gateway
   ```

   `every_cut_goes_through_commit_then_cut` fails (ten cuts, no helper), `a_replay_cursor_goes_up_only_with_a_bound_session_id` fails (the cursor goes up alone), and `a_host_revoke_cuts_its_streams_without_waiting_for_its_connection` fails (the stream is cut only after the 10 s wait). `a_revoke_committed_during_the_resolve_ends_the_request` and `a_revoke_while_a_json_answer_is_read_ends_the_request` pass: they pin what Task 3 built (probes 118, 119).

   ```sh
   git add -A && git commit -m "test: a revoke during the resolve or a JSON read ends the request, cuts only after commits, a host revoke cuts at once, a replay cursor needs a session id"
   ```

- [ ] **Step 3: Implement**

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
  /// goes up unwrapped (`SessionIds`, plan 8e decision 13).
  const FORWARDED_REQUEST_HEADERS: &[&str] = &["accept", "mcp-protocol-version", "last-event-id"];
  ```

with:

  ```rust
  /// goes up unwrapped (`SessionIds`, plan 8e decision 13), and
  /// `Last-Event-ID` only beside it (`LAST_EVENT_ID`).
  const FORWARDED_REQUEST_HEADERS: &[&str] = &["accept", "mcp-protocol-version"];

  /// A replay's cursor, passed up only with a session id this token may use
  /// (plan 8e, the security review's finding 4): an upstream that replays by
  /// event id alone would otherwise replay another session's stream to any
  /// token on the connection.
  const LAST_EVENT_ID: &str = "last-event-id";
  ```

In `crates/hennery-gateway/src/proxy.rs`, replace:

  ```rust
          out.insert(SESSION_ID, session.clone());
      }
  ```

with:

  ```rust
          out.insert(SESSION_ID, session.clone());
          for value in downstream.get_all(LAST_EVENT_ID) {
              out.append(LAST_EVENT_ID, value.clone());
          }
      }
  ```

In `crates/hennery-gateway/src/revocation.rs`, replace:

  ```rust
  //! fleet parent's ruling of 2026-10-02): scope that is checked only when a
  //! request arrives is not revocation. Every request the proxy serves on a
  ```

with:

  ```rust
  //! fleet parent's ruling of 2026-10-02): for a token's revoke, scope that
  //! is checked only when a request arrives is not revocation. (A change to a
  //! connection, an unmount, delete or edit, is refused at request time only,
  //! gateway spec §3.2.) Every request the proxy serves on a
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
  /// entry. In this order: the registry refuses its `hello`s from now on,
  /// its live connection is closed and gone, and only then are its sessions
  /// parked, so no reconciliation on that connection can bring them back.
  ```

with:

  ```rust
  /// entry. In this order: the registry refuses its `hello`s from now on, its
  /// gateway tokens are revoked and their streams cut, its live connection is
  /// closed and gone, and only then are its sessions parked, so no
  /// reconciliation on that connection can bring them back.
  ```

In `crates/hennery-sessions/src/hosts.rs`, replace:

  ```rust
          Ok(Revoke::Revoked | Revoke::AlreadyRevoked) => {}
          Err(err) => return internal(err),
      }
      if !state.hub.disconnect_and_wait(&host_id, REVOKE_DISCONNECT_BOUND).await {
  ```

with:

  ```rust
          Ok(Revoke::Revoked | Revoke::AlreadyRevoked) => {}
          Err(err) => return internal(err),
      }
      // Its gateway tokens, with what is open on them, at once (plan 8e; the
      // security review's finding 2): not only after the wait below, which a
      // connection that does not close holds up for its whole bound.
      // On failure it goes on: the disconnect is the stronger control, and
      // `on_host_revoked` revokes the tokens again below (the
      // re-confirmation's note 2).
      if let Err(err) = state.store.revoke_host_tokens(&host_id) {
          tracing::error!(%host_id, "revoking the host's gateway tokens at once failed: {err:#}");
      }
      if !state.hub.disconnect_and_wait(&host_id, REVOKE_DISCONNECT_BOUND).await {
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust

  /// `Store::close_now`'s body, inside the caller's transaction, with the
  ```

with:

  ```rust

  /// Commit `tx`, then end what is open on the tokens it invalidated (plan 8e
  /// decision 12): never before, or a rollback would leave a token working
  /// with its streams cut, and a request arriving between the cut and the
  /// commit would hold a fresh watch nothing cuts. The one place the sessions
  /// module cuts (`tests/cut_after_commit.rs` holds it to that).
  fn commit_then_cut(tx: Transaction<'_>, mcp: &dyn SessionMcp, cut: Cut) -> Result<()> {
      tx.commit()?;
      mcp.cut(cut);
      Ok(())
  }

  /// `Store::close_now`'s body, inside the caller's transaction, with the
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust

      /// The gateway, or the stand-in that gives nothing.
  ```

with:

  ```rust

      /// A revoked host's tokens, and what is open on them, at once (plan 8e,
      /// the security review's finding 2): the host revoke route runs this
      /// before it waits for the host's connection to close. Idempotent: the
      /// revoke's own `revoke_host` revokes what is left, none.
      pub fn revoke_host_tokens(&self, host_id: &str) -> Result<()> {
          let mut conn = self.conn();
          let tx = conn.transaction()?;
          let mcp = self.mcp();
          let cut = mcp.revoke_host_in(&tx, host_id)?;
          commit_then_cut(tx, mcp.as_ref(), cut)
      }

      /// The gateway, or the stand-in that gives nothing.
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
              self.mcp().revoke_in(&tx, id)?
          } else {
              Cut::default()
          };
          tx.commit()?;
          self.mcp().cut(cut);
          Ok(())
      }

      /// Like `mark_failed`, for a resume whose request failed: only a
  ```

with:

  ```rust
              self.mcp().revoke_in(&tx, id)?
          } else {
              Cut::default()
          };
          commit_then_cut(tx, self.mcp().as_ref(), cut)?;
          Ok(())
      }

      /// Like `mark_failed`, for a resume whose request failed: only a
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          };
          tx.commit()?;
          self.mcp().cut(cut);
          Ok(())
  ```

with:

  ```rust
          };
          commit_then_cut(tx, self.mcp().as_ref(), cut)?;
          Ok(())
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let cut = cut.and(mcp.revoke_in(&tx, session_id)?);
          tx.commit()?;
          mcp.cut(cut);
          self.remove_files(&conn, &dropped);
  ```

with:

  ```rust
          let cut = cut.and(mcp.revoke_in(&tx, session_id)?);
          commit_then_cut(tx, mcp.as_ref(), cut)?;
          self.remove_files(&conn, &dropped);
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let (events, cut) = close_in(&tx, &self.owner, mcp.as_ref(), session_id)?;
          tx.commit()?;
          mcp.cut(cut);
          Ok(events)
  ```

with:

  ```rust
          let (events, cut) = close_in(&tx, &self.owner, mcp.as_ref(), session_id)?;
          commit_then_cut(tx, mcp.as_ref(), cut)?;
          Ok(events)
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          };
          tx.commit()?;
          mcp.cut(cut);
          Ok(events)
  ```

with:

  ```rust
          };
          commit_then_cut(tx, mcp.as_ref(), cut)?;
          Ok(events)
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          self.mcp().cut(cut);
  ```

with:

  ```rust
          commit_then_cut(tx, self.mcp().as_ref(), cut)?;
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let cut = mcp.revoke_in(&tx, session_id)?;
          tx.commit()?;
          mcp.cut(cut);
          Ok(Reassign::Done(event))
  ```

with:

  ```rust
          let cut = mcp.revoke_in(&tx, session_id)?;
          commit_then_cut(tx, mcp.as_ref(), cut)?;
          Ok(Reassign::Done(event))
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          let cut = mcp.revoke_host_in(&tx, host_id)?;
          tx.commit()?;
          mcp.cut(cut);
          Ok(events)
  ```

with:

  ```rust
          let cut = mcp.revoke_host_in(&tx, host_id)?;
          commit_then_cut(tx, mcp.as_ref(), cut)?;
          Ok(events)
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          };
          tx.commit()?;
          mcp.cut(cut);
          Ok(Ingested { events: created, edge })
  ```

with:

  ```rust
          };
          commit_then_cut(tx, mcp.as_ref(), cut)?;
          Ok(Ingested { events: created, edge })
  ```

In `crates/hennery-sessions/src/store.rs`, replace:

  ```rust
          tx.commit()?;
          mcp.cut(cut);
  ```

with:

  ```rust
          commit_then_cut(tx, mcp.as_ref(), cut)?;
  ```

- [ ] **Step 4: Run the checks**

   Run the five checks:

   ```sh
   nix develop -c cargo fmt --all --check
   nix develop -c cargo clippy --workspace --all-targets --locked -- -D warnings
   nix develop -c cargo clippy -p hennery --locked -- -D warnings
   nix develop -c cargo test --workspace --locked
   nix develop -c cargo run -p hennery-proto --bin gen -- --check
   ```

- [ ] **Step 5: Commit**

   ```sh
   git add -A && git commit -m "fix: cut only through commit_then_cut, a host revoke's tokens cut at once, Last-Event-ID only with a bound session id"
   ```

### Task 9: The spec write-back

**Files:** Modify `docs/specs/2026-09-26-mcp-gateway-design.md` (§3.1, §3.2, §3.4, §5.2, §5.5, §5.7), `docs/specs/2026-09-26-acp-core-design.md` (§1, §3, §4.8, §8, migrations), `docs/specs/2026-09-26-kernel-design.md` (§4, §5.5).
- [ ] **Step 1: Write back the specs**

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
  *Built so far:* the `hennery-gateway` crate exists (plan 8a): connections,
  mounts and static credentials at rest, and their API. No `SessionMcp` trait
  yet (plan 8e), so `hennery-sessions` does not depend on the gateway; the
  binary merges the two routers side by side.
  ```

with:

  ```markdown
  *Built so far:* the `hennery-gateway` crate (plans 8a–8d): connections,
  mounts and static credentials at rest, their API, and the proxy. Plan 8e
  built `SessionMcp` as below, amended: the sessions store calls it inside its
  own transitions' transactions (gateway spec §3.1, §3.2).
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ````markdown
      /// Mints the per-session gateway token and returns the servers to pass
      /// in `session/new` / `session/load` for this session.
      fn servers_for(&self, host_id: HostId, hat_id: HatId, session_id: SessionId)
          -> Result<Vec<McpServerSpec>>;
      /// Revokes the session's token (park, close, adapter exit, host revoke).
      fn revoke(&self, session_id: SessionId);
  }
  ```
  ````

with:

  ````markdown
      /// In the start's or resume's transaction: for a delivering `mode`,
      /// mints the per-session gateway token and returns the servers to pass
      /// in `session/new` / `session/load`; otherwise none, and the previous
      /// token revoked. An error rolls the transition back.
      fn servers_in(&self, tx: &Transaction<'_>, session: SessionRef<'_>, mode: McpSessionDeliveryMode)
          -> Result<Delivered>;
      /// Revokes the session's token in the caller's transaction (park,
      /// close, adapter exit, re-assignment, delete, a failed start or resume).
      fn revoke_in(&self, tx: &Transaction<'_>, session_id: &str) -> Result<Cut>;
      /// Revokes every live token of a host (host revoke).
      fn revoke_host_in(&self, tx: &Transaction<'_>, host_id: &str) -> Result<Cut>;
      /// Ends what is open on the tokens a committed transaction invalidated.
      fn cut(&self, cut: Cut);
      /// The gateway's part of a hat's purge (kernel §5.5).
      fn purge_hat(&self, hat_id: &str) -> Result<()>;
  }
  ```

  The sessions module decides the delivery (umbrella §8.5) and passes it in;
  it owns `sessions.hat_id`.
  ````

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
    collector sends no servers until the gateway mints them (plan 8e).
  ```

with:

  ```markdown
    collector sends servers, minted by the gateway, since plan 8e, only to a
    host that announces this capability.
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
  `_meta`. The collector sends no servers until plan 8e.
  ```

with:

  ```markdown
  `_meta`. Since plan 8e the collector sends the session's servers
  (gateway spec §3.2).
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
    and `session_closed`, on adapter exit, on a close of an unattached session
    and on host revoke (`SessionMcp::revoke`). A presumed park
  ```

with:

  ```markdown
    and `session_closed`, on adapter exit, on a close of an unattached session,
    on host revoke, on re-assignment, on delete and on a start or resume the
    route fails (`SessionMcp::revoke_in`, `revoke_host_in`), in the
    transition's transaction; what was open on it is cut once that commits
    (gateway spec §3.1). A presumed park
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
     Codex app-server timeouts in a row, for the bounded hybrid (9d-ii).
  ```

with:

  ```markdown
     Codex app-server timeouts in a row, for the bounded hybrid (9d-ii);
  16. `sessions.mcp_delivery_mode`, `mcp_delivery_servers` and
     `mcp_delivery_at`: what the latest start or resume was given, the mode
     and a count (plan 8e decision E10). A delete scrubs them.
  ```

In `docs/specs/2026-09-26-acp-core-design.md`, replace:

  ```markdown
    verbatim payloads) is open, for the maintainer before plan 8e.
  ```

with:

  ```markdown
    verbatim payloads) was the maintainer's open question; plan 8e took the
    default, reversible: a session token in an ACP payload is stored and
    published redacted by shape (gateway spec §3.1, plan 8e decision 11).
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
    is the last accepted `hello`, not liveness; no rename or default hat yet.
  - One live connection per host (ACP core §3.5).
  ```

with:

  ```markdown
    is the last accepted `hello`, not liveness; no rename or default hat yet.
    Since plan 8e it also keeps the `hello`'s per-agent `mcp_isolation`
    (`hosts.mcp_isolation`, JSON, `NULL` until such a `hello`), so the host list
    shows each agent's MCP delivery while the host is away (`HostItem`, plan 8e
    decision E7).
  - One live connection per host (ACP core §3.5).
  ```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

  ```markdown
  *Built so far:* `on_host_revoked` and `on_hat_purged`, implemented by the sessions module; the gateway's purge hook is plan 8's.
  ```

with:

  ```markdown
  *Built so far:* `on_host_revoked` and `on_hat_purged`, implemented by the sessions module. The gateway's part of a purge (its connections with their credentials and mounts, its stdio sets, its session tokens, their streams cut) runs through `SessionMcp::purge_hat`, first: the purge route and the hook owe the purge's checkpoint, then run the gateway's part, then the sessions' (plan 8e; A15), so a hat stuck frozen cannot reach MCP meanwhile, and a failed gateway part stops the purge before any session is deleted.
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
  | session | automatically, by `SessionMcp::servers_for` at every start and resume of a session (ACP core §1) | connections of the session's hat mounted on the session's host |
  ```

with:

  ```markdown
  | session | automatically, by `SessionMcp::servers_in` inside every start and resume of a session (ACP core §1) | connections of the session's hat mounted on the session's host |
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
  - **One token per session.** It is revoked on park, close, adapter exit and
    host revoke (`SessionMcp::revoke`), and superseded by the token minted at the
    next resume. A presumed park while the host is merely offline does not revoke
    it (ACP core §4.8).
  ```

with:

  ```markdown
  - **One token per session.** It is revoked on park, close, adapter exit,
    host revoke, re-assignment to another hat, a start or resume the route
    fails, and delete (`SessionMcp::revoke_in`, `revoke_host_in`), each in
    the transition's own transaction; it is superseded by the token minted at
    the next resume, and a hat's purge deletes it. A presumed park while the
    host is merely offline does not revoke it (ACP core §4.8), nor does a
    host's report about a session that a newer resume has since superseded.
  - **A revoke ends what is open on the token** (plan 8e decision 12; the
    fleet parent's ruling of 2026-10-02): for a token's revoke, scope checked
    only when a request arrives is not revocation. Every request the proxy
    serves on a session
    token watches that token from before it is resolved until its answer's
    body ends; a revoke, a supersession and a purge, once their transaction
    has committed, cut every watch on the tokens they invalidated: a request
    not yet answered ends, and a stream open on it is cut. Cutting before the
    commit would race a rollback; registering the watch before the resolve
    closes the race with a revoke in flight. Another token's streams are
    untouched. The sessions module cuts in one place only, after the commit
    (`commit_then_cut`, held by a source audit). A host revoke revokes and
    cuts its tokens at once, before it waits for the host's connection to
    close. A change to a connection (an unmount, a delete, an edit) is
    refused at request time only (§3.2): what is already open on it runs
    until the client or the upstream ends it (open for the maintainer).
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
    appear in timeline events or SSE (ACP core §8).
  ```

with:

  ```markdown
    appear in timeline events or SSE (ACP core §8). A session token the agent
    printed into an ACP payload is stored and published redacted, by shape:
    every run of `hnry_session_` (any case) and at least 8 hexadecimal digits
    (plan 8e decision 11, the default of the maintainer's open question Q2 of
    plan 8c, reversible). A token split across two updates, or encoded
    otherwise, is not caught; it stops working at the session's next park.
    Two object keys that redact alike are merged into one.
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
     §3.3) it calls `SessionMcp::servers_for(host, hat, session)`, which mints the
     session token and returns every connection of the session's hat mounted on
     the session's host, as
     `{type: "http", name: "hennery-<slug>", url: "<public_url>/mcp/<slug>",
     headers: [{name: "Authorization", value: "Bearer <session token>"}]}`,
     followed by the stdio servers for that (host, hat) (§3.4).
  ```

with:

  ```markdown
     §3.3) it decides the delivery (umbrella §8.5;
     `hennery_proto::rest::mcp_session_delivery`, the one mapping the host list
     reads too, plan 8e decision E8) and, inside the start's or resume's own
     transaction, calls `SessionMcp::servers_in(tx, session, mode)`. For a
     mode that delivers, it mints the session token (a failed mint rolls the
     transition back) and returns every connection of the session's hat
     mounted on the session's host, as
     `{type: "http", name: "hennery-<slug>", url: "<public_url>/mcp/<slug>",
     headers: [{name: "Authorization", value: "Bearer <session token>"}]}`,
     followed by the stdio servers for that (host, hat) (§3.4). For any other
     mode it returns none, and the previous token is revoked. What was decided
     is recorded on the session, `SessionDetail.mcp_delivery`: the mode and a
     count, never a server, header or token (plan 8e decision E10).
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
  authentication (kernel §3.4), since they are commands the host will execute.

  ```

with:

  ```markdown
  authentication (kernel §3.4), since they are commands the host will execute.

  As built (plan 8e decisions E1–E6): a set is replaced whole (`PUT`), and a
  server keeps its row while its name stays; names follow the slug rules, are
  unique in their set and disjoint from every connection slug of the owner;
  env values are write-only (a `GET` returns each name and whether it has a
  value); `command` and `args` are returned as stored, not sealed (secrets
  belong in env); a revoked host's sets stay readable and refuse a `PUT` (409
  `host_revoked`); a hat's purge deletes its sets; standalone mode has no stdio
  routes. A host without the `mcp_servers` capability is given none.

  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
    `Mcp-Protocol-Version`, `Last-Event-ID`. `Authorization` is replaced by the
  ```

with:

  ```markdown
    `Mcp-Protocol-Version`, and `Last-Event-ID` only beside a session id bound
    to the token (plan 8e: an upstream that replays by event id alone would
    otherwise replay another session's stream; one that ignores
    `Mcp-Session-Id` altogether can still do so, which the gateway cannot
    prevent). `Authorization` is replaced by the
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
  - **Sessions:** `Mcp-Session-Id` passes through in both directions, so each
    downstream client session maps to its own upstream session and the gateway
    holds no session table. `DELETE` is forwarded so client terminations reach
    the upstream. *(G-16: the predecessor answered DELETE with 405, leaving
  ```

with:

  ```markdown
  - **Sessions:** each downstream client session maps to its own upstream
    session, and the gateway holds no session table: the `Mcp-Session-Id` it
    sends down is the upstream's, bound to the token and the connection (plan
    8e decision 13), `<upstream id>.<tag>`, `tag` the lowercase hex of an
    HMAC-SHA256 under a key of the process's own over the token's hash, the
    connection's id and the upstream id, each length-prefixed. A request's id
    is verified and stripped (split at its last `.`) before it goes up; one
    unbound or tagged for another token or connection, or more than one, is
    the same 404 as an unknown token, and nothing is forwarded. An upstream
    answer with more than one id is 502 `upstream_invalid`. Every token on a
    connection uses the same upstream credential, so without this a token
    presenting another session's id would get what the upstream serves for
    it. The key is per process: after a restart every id is refused and the
    client initializes again (a 404 on a request with a session id means so
    in MCP's streamable HTTP). `DELETE` is forwarded so client terminations
    reach the upstream. *(G-16: the predecessor answered DELETE with 405, leaving
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
      thing that separates two sessions' streams: the gateway forwards the
      client's, does not bind it to the token, and every token on a connection
      uses the same upstream credential, so a token presenting another
      session's id gets what the upstream serves for it (open for 8e).
  ```

with:

  ```markdown
      thing that separates two sessions' streams, and the gateway binds it to
      the token and the connection (§5.2, plan 8e decision 13): a token cannot
      present another session's id.
  ```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

  ```markdown
      answer in JSON only when done).
  - The egress client is chosen at every request from the connection's
  ```

with:

  ```markdown
      answer in JSON only when done).
    - A JSON answer, read whole to be judged (§5.3), must arrive whole within
      60 s of its head; past it the answer is 502 `upstream_unreachable` and
      nothing of it goes down (plan 8e decision 14). An event stream has no
      such deadline.
  - The egress client is chosen at every request from the connection's
  ```

- [ ] **Step 2: Commit**

   ```sh
   git add -A && git commit -m "docs(spec): write back plan 8e: sessions get their MCP servers, revokes end streams, bound session ids"
   ```

## After this plan

- **Plan 8f (OAuth)**: an OAuth connection is delivered as any other mounted connection (Task 3's `servers_in`); nothing session-side changes.
- **Plan 8h (Codex's composed home)**: a new isolation value maps through `McpAgentDelivery::of` (E8), and `mixed` (decision 2) is computed for it to read; until then E9's unknown values read as `default_hat_only`.
- **Plan 8h** also owes a positive test and a revert-probe for `mixed` once it can change an outcome (the security review's finding 8).
- **Open for the maintainer** (the security review's product questions): whether a connection's unmount, delete or edit cuts the streams open on it, to be decided before the frontend offers deleting a connection or rotating its credential (the re-confirmation's note 3); whether stdio env values an agent echoes are redacted by value (with Q2).
- **Lane L20**: when `GatewayStore::hats_with_credentials()` lands (frontend 4d), it must count (host, hat) stdio sets with any stored env value, with a test and a probe.
- **Operator items:**
  - After a collector restart every live MCP session's id is refused (404), and the agents' MCP clients must initialize again (decision 13; lane L10). A live check with the pinned adapters.
  - Q2 (decision 11) stays open: redaction is the default, reversible.
  - The process-list exposure of the token (gateway §3.2) is unchanged.
- **Frontend**: `HostItem.mcp_delivery`, `SessionDetail.mcp_delivery` and the stdio routes are ready for the mount grid and the session view (`mcp_stdio` capability on).

Generated with Claude AI — please review before distribution.
