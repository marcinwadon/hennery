# MCP gateway (plan 8d): the proxy and session tokens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** the gateway's proxy, `POST|GET|DELETE <public_url>/mcp/<slug>`, for connections with a static credential or none, and the session tokens it is reached with. With it:
- a session token, minted and revoked inside the sessions store's own transactions (lane L1), resolves to "session S on host X, hat H", and reaches only hat H's connections mounted on host X;
- a request goes upstream with the connection's credential and an allowlist of headers, through the kernel's egress policy, and its answer streams back chunk by chunk;
- the tool allowlist, the capabilities not forwarded, the 401 rule, the content-type rule, the body caps and the per-connection limits are the gateway's (gateway spec §5).

Plan 8e wires the token primitives into `hennery-sessions`; 8f adds OAuth behind the 401 seam; 8g adds standalone clients behind `ClientIdentity` and `MountPolicy`.

**Architecture:** four new modules in `hennery-gateway`, one migration, one route in the collector.
- `tokens`: the `gw_session_tokens` primitives, each taking the caller's `&rusqlite::Transaction` (`mint_in`, `revoke_in`, `revoke_host_in`, `purge_hat_in`), and `SessionToken`, which zeroizes and never shows itself.
- `scope`: `Principal`, the `ClientIdentity` and `MountPolicy` traits (umbrella §10.2), and `ProxyStore`, the full-mode implementation of both on a `hennery.db` connection of its own, with the two status writes live traffic makes (gateway spec §7).
- `jsonrpc` (private): what the proxy reads of JSON-RPC. The duplicate-key check, the `tools/call` refusal, `initialize`'s capabilities, the `tools/list` filter, and server-to-client requests refused, in JSON and per SSE event.
- `proxy`: the axum route, hand-written on reqwest through `hennery_kernel::egress` (plan 8b), with `Limits` and `ProxyState::full`.
- The collector builds one `Egress` and merges `proxy::router` beside the operator's routes, outside `operator_only` (lane L8).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), axum 0.8, reqwest 0.12 (through the kernel's egress clients), rusqlite, serde_json, futures, tokio. **No new crate:** the gateway takes the workspace's `futures`, `reqwest`, `serde` and `tokio`, so `Cargo.lock` gains three lines in its stanza (`tokio` was a dev-dependency already). `reqwest` is a direct dependency only for `reqwest::Body`, which `egress` does not re-export; every request still goes out through `EgressClient`.

**Spec:**
- Gateway §3.1: "Tokens are 32 random bytes, stored only as SHA-256 hashes … One token per session. It is revoked on park, close, adapter exit and host revoke … and superseded by the token minted at the next resume … Scope is checked at request time from the token's (host, hat) and the mounts as they are now … An unknown token, a revoked token, an unmounted connection, or a connection of another hat all return 404."
- Gateway §5.1–§5.7 (the proxy) and §7 ("Live traffic also updates status: a 2xx through a connection marked `needs_auth`/`error` sets `ok`").
- Gateway §11: the streaming, 401, egress, scope, token-hygiene, allowlist and capabilities tests.
- Kernel §3.3: "`/mcp/*` (gateway proxy) | Bearer token | Exempt"; kernel §7: "Response compression for JSON and HTML only; never on SSE or the gateway proxy routes."
- Umbrella §10.1 (the proxy's invariants) and §10.2 (`ClientIdentity`, `MountPolicy`).

It builds on the lane's decisions L1 (mint and revoke inside the transition's transaction), L6 (owner and composite foreign keys; session ids opaque), L7 (loopback tests under `internal_network`, no test-only bypass), L8 (outside `operator_only`, no compression, token hygiene asserted on captured logs) and L11 (no full upstream URL in logs, errors or `Debug`), and on plan 8b's obligations for the proxy ("After this plan").

**Base:** `main` at `e4e2ca3`: plan 8a merged (PR #77: the crate, its store, the master key, the connections API, its documented wire types), 8b and 8b-ii (`hennery_kernel::egress`, plain `http` to internal addresses under the marking) and 10b-ii (the collector's one `Egress`, for Web Push). Anchors are taken from `e4e2ca3`. The plan uses 8a's and 8b's APIs as they are, and changes three lines of 8a's (decision 15).

**Status:** written 2026-10-02. Amended after the security review of 2026-10-02 (approve after amendments: B1, B2 and O1–O8 taken, O4 accepted) and two scoped re-confirmations (the first found one new bypass, taken; the second: confirmed with notes); then at execution by the four task reviews and the whole-branch review (request changes: one blocker, taken) with its scoped re-confirmation (approve after one amendment, taken), and a scoped re-confirmation of those fixes (approve after amendments, taken). Its product question Q1 was decided by the lane parent: §5.6 stands, event streams only; one more product question is open for the maintainer (open streams after a revoke, "After this plan").

**How the code blocks were made and checked:**
- Every block below was generated from the code on `scratch/gateway-8d-7`, split into this plan's commits.
- The plan was replayed from its own text onto `e4e2ca3`, in a scratch worktree. The extractor applied blocks to 21 files over the 7 commits, and the tree matched the code it was generated from byte for byte.
- The five checks passed: 1301 tests, from 1243.
- The 101 revert-probes of Tasks 1–3 were run, and each failed as expected (Task 1's and Task 2's Step 6, Task 3's Step 5); two more were measured inert and are kept (Task 2's Step 6).

## Execution status

_Not executed yet._

## Scope

**In:**
- `gw_session_tokens` and its primitives for plan 8e, with direct tests;
- `ClientIdentity`, `MountPolicy` and their full-mode implementation;
- the proxy for `none` and `static` connections: authorization, forwarding, streaming, the 401 rule, the allowlist, the capabilities, the egress policy and the limits (gateway spec §5);
- the collector serving it;
- the gateway spec written back with the decisions below (Task 4).

**Out:**
- OAuth, its refresh and retry (8f): the proxy answers an OAuth connection 502 `upstream_auth` until then, and `refreshed` is the one place 8f adds to;
- standalone clients and pins (8g): `PrincipalKind` has one variant now;
- the sessions wiring (8e): nothing calls `mint_in` or the revokes yet, so no session can reach the proxy until 8e;
- the `Notifier` (8f): the status writes here are where its transitions start;
- checking a connection's URL with `check_url` when the operator saves it (plan 8b's hand-off to "8a/8d"): it is a change to 8a's create and update; the proxy checks every request's URL anyway (decision 15).

## Decisions this plan makes where the spec is silent

The security review of 2026-10-02 confirmed decisions 1–3, 6, 7, 9, 11–13 and 15–16 on the maintainer's behalf, and 4, 5, 8, 10 and 14 after its amendments; 17 and 18 came from its findings, and decision 5's last rule from the first re-confirmation; 19 and the amendments marked "the Task 2 review" came from the opus review of Task 2's execution. Its product question Q1 was decided by the lane parent: §5.6 stands ("After this plan"). Decision 6's case rule and 14's re-read came from the whole-branch review and its re-confirmation.

1. **A session token's shape:** `hnry_session_` and 64 lowercase hexadecimal digits, 32 bytes from the OS's CSPRNG.
   - The prefix lets a secret scanner (GitHub's, a pre-commit hook) recognise a leaked token, and tells the token kinds apart: plan 8g's client tokens take a prefix of their own (`hnry_client_` proposed), so `ClientIdentity` can dispatch on it.
   - Stored as the SHA-256 of the whole token, hex, `UNIQUE`. A token of any other shape is not looked up.
   - `SessionToken` is `Zeroizing<String>` inside, its `Debug` shows `<redacted>`, and `expose()` is the only way to the plaintext, for the frame 8e builds.
2. **The table** (lane L6): `gw_session_tokens(session_id PK, owner_id, host_id, hat_id, token_hash UNIQUE, created_at, last_used_at, revoked_at)`, with composite foreign keys to `hosts(id, owner_id)` and `hats(id, owner_id)`, none to sessions.
   - `mint_in` upserts the session's row, replacing its hash, host, hat and `created_at` and clearing `last_used_at` and `revoked_at`, so the previous token stops resolving (superseded). It never takes over another owner's row: zero rows changed is an error. A host or hat that is not the owner's fails the mint, and so the caller's transition.
   - `revoke_in` and `revoke_host_in` set `revoked_at` and keep the row; a revoke of nothing is not an error, and returns false or 0.
   - `purge_hat_in` deletes the hat's tokens, revoked ones too: the hat foreign key would otherwise keep the hat row.
   - A session's delete (the purge lane) is a revoke today; deleting the row is a primitive for 8e or 9 to add ("After this plan").
3. **A connection with nothing to send sends nothing:** a `static` connection without a token, and an OAuth one before 8f, answer 502 `upstream_auth` ("connection <label> needs re-authorization in hennery") without contacting the upstream.
4. **The content-type rule's edges** (§5.2):
   - `application/json` and `text/event-stream` pass, in any case and with parameters.
   - The `Content-Type` sent down is the gateway's: exactly the type it judged the body by, without parameters. An upstream's own header is never forwarded, and more than one `Content-Type` is 502 (the review's B1). Otherwise `application/json; x=text/event-stream`, or a JSON header before an SSE one, would pass unread as JSON while a client that matches with `includes("text/event-stream")` parses an event stream the gateway never looked at.
   - A body-less answer (202, 204, or `Content-Length: 0`) without a type passes, empty: a notification's 202 and a `DELETE`'s 204 must work.
   - A body without a type, any other type, and any `Content-Encoding` but `identity` are 502 `upstream_content_type`, whatever the status. The proxy asks for `Accept-Encoding: identity`.
   - Accepted cost: an upstream that answers a 404 or 405 with `text/plain` is seen as 502. MCP SDKs answer those in JSON.
5. **Event streams are passed on event by event** (§5.3, §5.5, §5.6).
   - Each complete event goes on as soon as its chunk arrives. A partial event is held until its end, since a client can use no partial event and the gateway may have to rewrite or drop it.
   - Events that no rule touches pass byte for byte. A rewritten event keeps its other fields (`id:`, `event:`) and gets one `data:` line.
   - One event is at most 8 MiB, the cap the spec sets for a filtered `tools/list`: past that without its end, the stream ends with an error. The scan for an event's end goes on from where the last chunk's stopped, so a long event costs linear time (the review's O2).
   - **It fails closed** (the review's B2): a byte-order mark at the stream's start is dropped, as a WHATWG parser drops it; an event whose `data` is not JSON to serde_json (a lone surrogate, `1e400`, plain text) is dropped and logged, since a client's parser may read what the gateway cannot; so is an event whose `data` has a key twice in an object (the Task 2 review: serde_json keeps the last, a client may keep the first, decision 6's reason going down), and an event with a line that starts with a byte-order mark, wherever it is (the re-confirmation's note 1: once earlier events are dropped, it could be the first thing the client reads, and its parser strips the mark); a last event the stream never ends is dropped, as a client drops it. Accepted (the re-confirmation's note 2): a stream that ends in `…\n\r` loses its last event, which a WHATWG parser would dispatch; that fails closed. Events without data (comments, pings) still pass byte for byte.
   - The first-chunk guarantee holds for every chunk that ends an event; the test sends a JSON chunk and a complete SSE event, and both must arrive within 5 s while the upstream blocks.
6. **A `POST` body must be JSON with no key twice in any object**, or 400 `invalid_request`, and nothing goes up.
   - Why: the allowlist reads `method` and `params.name` with serde_json, which keeps the last of two keys. An upstream parser that keeps the first, or accepts what serde_json refuses (a trailing comma, JSON5), would run a call the check never saw.
   - **Nor may it spell a key the gateway reads otherwise than exactly** (the whole-branch review's blocker): Go's `encoding/json` matches names whatever their case, folding `ſ` to `s` too, so `{"METHOD":"tools/call",…}` or a `"NAME"` beside `"name"` reached a Go upstream as a call the allowlist never saw. Folding also ignores `_` and `-`, as Go's `encoding/json/v2` does when an upstream asks it to match names case-insensitively (the re-confirmation's F1). Refused, 400, at the levels the gateway reads: a message's `jsonrpc`, `id`, `method` and `params`; a `tools/call`'s `params.name`; an `initialize`'s `params.capabilities` and the capabilities not forwarded. Two other keys equal under folding elsewhere (a tool's arguments) are not the gateway's to judge, and pass. The Kelvin sign, the one other letter Unicode folds to ASCII, folds to `k`, which no name read here has. The same rule drops an event going down (decision 5), since a client's decoder may ignore case too.
   - The bytes go up as they came. Only an `initialize` that had a capability to strip is re-serialised. Their type goes up as `application/json`, which the gateway checked, never the client's (the review's O6).
   - `GET` and `DELETE` bodies are neither read nor sent.
7. **A batch holding a refused `tools/call`** is answered whole by the gateway, and nothing in it goes up: that call `-32602`, each other request `-32600` ("batch refused: a tools/call in it is not available through hennery"), notifications nothing; with nothing to answer, 202.
   - Why: forwarding the rest and merging the gateway's error into the upstream's answer, JSON or a stream, is fragile; MCP 2025-06-18 has dropped batches anyway.
   - A `tools/call` whose `params.name` is not a string is outside every allowlist.
8. **What live traffic does to a connection's status** (§7):
   - a 2xx with a JSON or event-stream body sets `ok` from any other status, `not_connected` included, since it proves the upstream takes what was sent. A body-less 202 or 204 proves nothing: some upstreams take a notification before they check its credential (the review's O5); nor does an empty body under a type (`Content-Length: 0`, the Task 2 review);
   - a final 401 sets a static or `none` connection `needs_auth` ("the upstream refused the credential (401)");
   - both only if the connection still has the URL the request went to and, for `ok` on a static one, still a token, so an answer about the old upstream never marks the new one;
   - nothing else changes status: one 500 for a bad tool call is not an outage. The probe's `error` (8f) is not live traffic's to set.
   - The writes are conditional (`status != …`), so a healthy connection costs no write per request.
9. **The egress client is chosen at every request from the connection's stored `internal_network` flag** (the parent's directive of 2026-10-02), read with the credential for a static connection, never from anything the request carries.
   - It is one small function, `egress_client`. Plan 8b-ii (merged) put plain `http` to internal addresses under `Allowance::InternalNetwork` itself, so nothing swaps in here.
   - Pinned by a test that sends headers claiming the internal network to a loopback `https` upstream: refused before any connection, until the operator marks the connection (a `PATCH` through the store; the next request goes).
   - Plan 8b-ii's obligations to the proxy, each met: the allowance from the stored flag only, in that one function; the flag and the URL from the same stored row on every request (the scope query for `none`, `static_credential`'s one statement for `static`), nothing cached across a `PATCH`; the stored scheme and authority sent verbatim (only `/mcp/{slug}` routes here, and the client's query is dropped, decision 11); the client's `Host` never forwarded (the request headers are an allowlist). Pinned: the upstream sees the stored authority as `Host` while the client sends another (probe `host-not-forwarded`), and a connection not marked internal is refused plain `http` to a LAN address by the egress policy, read from its log line since the 502 is the same as a failed connect (`tests/proxy_egress.rs`, probe `egress-plain-http-lan`).
10. **Limits** (§5.7; `Limits`, configurable):
    - 64 `POST` and `DELETE` requests in flight per connection. A permit is taken after scope, before the body is read, and held until the answer's body ends or the client goes; a `POST` answered with a stream counts here. A body has 30 s to arrive, or 408 `request_timeout` (the review's O3).
    - 32 open `GET` streams per connection, held for the stream's life. A `GET` takes no request permit, so open channels never starve requests (the review's O3).
    - Past either cap, 503 `busy`, and nothing is sent.
    - The response head must arrive within 300 s, since a long `tools/call` may answer in JSON only when done.
    - No idle timeout on streams: an MCP `GET` channel may rightly be quiet for hours. Open streams are counted, idle or not, stricter than §5.7's "idle" (plan 8b's decision 10).
    - The answer to a refused server request takes a request permit; at the cap it is skipped and logged.
11. **The upstream URL is the connection's, exactly:** the client's query string and anything after the slug are not forwarded.
12. **`last_used_at`** is written when the token is used and the stored value is a minute old or unset, not on every request.
13. **Every answer the proxy makes itself is an `ApiError`, `{code, message}`**, as 8a's and every other route's (the parent's directive of 2026-10-02); §5.4's `{"error": "upstream_auth", …}` is written back to `{"code": …}`. JSON-RPC errors inside an MCP answer stay JSON-RPC.
    - Out of scope is one byte-identical 404 `not_found`, never 401.
    - A slug that is not UTF-8 is the same 404, not axum's plain-text 400 (the whole-branch review).
    - A missing, malformed or repeated `Authorization` is 404 too: a 401 starts the agent's own OAuth (G-17).
    - Every answer, the proxy's own included, carries `nosniff` and `Cache-Control: no-store` (decision 19), from layers on the route.
    - Anything deeper under a slug (`/mcp/<slug>/…`, a bare trailing slash too) is the same 404, never another router's answer (the review's O7; the Task 2 review's finding 2).
    - So is any other method than `POST`, `GET` and `DELETE`, `HEAD` included: axum hands `HEAD` to the `GET` handler, which would send it upstream and take its empty JSON answer for proof the connection works (the Task 2 review's finding 1).
14. **Server-to-client requests refused** (§5.6): `sampling/createMessage`, `elicitation/create` and `roots/list`.
    - Each is answered `-32601` ("<method> is not available through hennery") by a `POST` to the same upstream session (its `Mcp-Session-Id`, from the answer or the request) with the connection's credential, in the background.
    - Its event is dropped; in a batch, the element is.
    - A notification of those names has no id and passes.
    - The answer keeps no client and no credential (plan 8b-ii's obligation, nothing cached across a `PATCH`; the whole-branch re-confirmation's amendment): each one reads the connection again through the principal's scope, and is not sent if the connection left it, is another one under the same slug, or has another URL or internal marking than when the stream opened. It goes with the credential and the egress client as they are now.
15. **What this plan changes of 8a's code, and why.**
    - `schema.rs` gets the second migration.
    - `tests/store.rs`'s `the_store_and_the_kernel_agree_on_the_owner_whichever_opens_first` pins the component's version: 1 becomes 2.
    - The owner audit (`owner_filter.rs`) lists `tokens.rs` and `scope.rs`.
    - `main.rs` merges the proxy's router beside 8a's.
    - Nothing of 8a's behaviour changes. 8a's `purge_hat` does not delete tokens: whoever wires `on_hat_purged` (lane L6) calls `tokens::purge_hat_in` too ("After this plan").
16. **Revoked hosts are out of scope in the reads themselves:** the token resolve and the scope query both join `hosts.revoked_at IS NULL`, as 8a's mount list does, so a revoke site 8e misses stays closed.
17. **A response's id matches a `tools/list` request's** when equal, or the same number however written (`1`, `1.0`), as a client's matching may read them (the review's O1). The `tools/call` refusal remains the enforcement; the filter hides.
18. **Store reads on the request's task** (the review's O4, accepted): the token resolve and the scope query are indexed point queries under the store's mutex, on the tokio worker, as 8a's API handlers do. A token-shaped string costs one SHA-256 and one lookup before any limit. Moving them to `spawn_blocking` is a later change if it shows.
19. **`Cache-Control: no-store` on every answer** (the Task 2 review, on the lane's question): §5.2 listed `Cache-Control` among the response headers forwarded. An upstream's `public` or `s-maxage` would make an answer to a request that carries a token storable by a shared cache in front of hennery (RFC 9111 §3.5), and the proxy's own 404s are cacheable by default (RFC 9110 §15.1) with no `Vary: Authorization`. So the upstream's `Cache-Control` is never forwarded, and a layer on the route sets `no-store` on every answer, as plan 8a's API does; MCP clients do not cache. Written back into §5.2.

## The security review's answers

**The security review (opus, 2026-10-02), on the maintainer's behalf: approve after amendments** (B1, B2), with one product question (Q1), since decided by the lane parent. It ran the gateway's 106 tests at the reviewed commit in a scratch worktree, and seven probe tests of its own, each asserting what the code did then; all seven passed, which confirmed B1, B2, Q1 and O1. It checked the rest by reading: tokens (CSPRNG, prefix, hash-only storage, indexed lookup, the upsert's owner guard, revoked hosts joined out, the owner filter on every statement, the purge failing closed), scope (nothing in the request widens it; one 404; no 401), headers both ways, the answerer (the connection's own URL, under the cap), the head timeout (`send_streaming` drops reqwest's whole-request deadline), `egress_client`, the logs, and the router.

| Finding | Taken how |
|---|---|
| **B1** The answer's `Content-Type` was judged by its first value cut at `;` and forwarded as it came: `application/json; x=text/event-stream`, or a JSON header before an SSE one, passed an event stream unread to a client that matches with `includes("text/event-stream")`, past §5.5's filter and §5.6's refusal | Taken: the upstream's `Content-Type` is never forwarded; the gateway sets exactly the type it judged (decision 4); two `Content-Type` headers are 502. Test `the_answer_s_type_is_the_one_the_gateway_judged`; probes `b1-one-type-down`, `b1-two-types` |
| **B2** The SSE rewrite failed open: a byte-order mark before the first event, data serde_json refuses but JavaScript reads (a lone surrogate), and an unended last event all went through raw | Taken: decision 5's fail-closed rules. Test `an_event_stream_fails_closed_on_what_it_cannot_read` and the `jsonrpc` unit tests; probes `b2-bom`, `b2-unreadable`, `b2-unended-tail` |
| O1 An answer to id `1` written `1.0` escaped the `tools/list` filter | Taken: decision 17. Probe `o1-same-id` |
| O2 A long event was rescanned from its start on every chunk | Taken: `event_end` resumes from the last line start (decision 5); the unit test cuts an event at every offset |
| O3 Slow request bodies held request permits with no deadline; a `GET` took a request permit too | Taken: 30 s, then 408 `request_timeout`; a `GET` takes only a stream permit (decision 10). Tests `a_body_that_does_not_arrive_in_time_is_408`, `a_get_stream_takes_no_request_permit`; probes `o3-body-timeout`, `o3-get-no-request-permit` |
| O4 Store reads on the tokio worker before any limit | Accepted, as decision 18 |
| O5 A body-less 2xx could clear `needs_auth` | Taken: decision 8. Probe `o5-bodiless-sets-nothing` |
| O6 The client's `Content-Type` went up | Taken: `application/json` (decision 6). Probe `o6-type-up` |
| O7 `/mcp/<slug>/…` fell through to another router | Taken: the same 404 (decision 13); "After this plan" tells the frontend. Probe `o7-deeper-404` |
| O8 Name `reassign_hat` in 8e's obligations | Taken ("After this plan") |
| **Q1** A server-to-client request inside a plain JSON answer passes (rmcp hands it to its handler) | **Decided by the lane parent** (2026-10-02): §5.6 stands, event streams only; a JSON body answering a `POST` is that request's response, not a server-initiated request ("After this plan") |

Per decision: 1–3, 6, 7, 9, 11–13 and 15–16 confirmed; 4 amended by B1; 5 by B2 and O2; 8 by O5; 10 by O3; 14 by B2, Q1 decided by the lane parent; 17 and 18 added from O1 and O4.

**The parent's directives of 2026-10-02**, taken during the review: every answer the proxy makes itself is an `ApiError` (decision 13, and §5.4 written back); the egress client is chosen from the stored flag in one function (decision 9); the base is `main` after plan 8b's merge.

**The re-confirmation (a fresh opus reviewer, 2026-10-02), scoped to the amendments: not confirmed, for one new bypass in B2's handling; every other item confirmed.** It ran the gateway's tests at the amended commit in a scratch worktree, and a probe of its own, which reproduced the bypass. Its notes:
1. **(blocking)** A byte-order mark that would start the client's stream (behind a stripped mark, or behind an event the gateway dropped) let a `sampling/createMessage` event through: the gateway read the line as an unknown field and passed the event as it came; the client strips the mark and dispatches it. **Taken:** an event with a line that starts with a mark is dropped, wherever it is (decision 5). Both inputs are in `an_event_stream_fails_closed_on_what_it_cannot_read`; probe `bom-line`.
2. A stream ending `…\n\r` loses its last event, which a WHATWG parser would dispatch. Accepted, fails closed (decision 5).
3. Invalid UTF-8 in an event no rule touches goes on as it came; clients decode it with replacement characters. Written into §5.3.
4. No desync from dropped events was found; an event of exactly 8 MiB passes, anything larger ends the stream.

**The second re-confirmation (a fresh opus reviewer, 2026-10-02), scoped to note 1's fix: confirmed with notes.** It ran the gateway's tests at the fixed commit in a scratch worktree. It found the bypass closed: the gateway emits only whole events that come back unchanged or rewritten, the check runs before an event is parsed, a rewritten event starts with a line that passed it or with `data: `, and a lone empty line first dispatches nothing. Its notes: the drop's log line and `Unreadable`'s doc still named only data that is not JSON (taken: both now say "cannot read"); and a client splitting lines on characters beyond CR and LF (Python's `str.splitlines`) would read lines otherwise than the gateway. That one is accepted: MCP's clients parse SSE by WHATWG's rules, CR and LF only.

**The task reviews and the whole-branch review (opus, 2026-10-02), at execution.** Each task's reviewer approved with fixes, all taken: Task 1's two untested guards (the mint's owner guard, `last_used_at` cleared on a re-mint) and the token built at its final size; Task 2's `HEAD` sent upstream and taken for proof (decision 13), `/mcp/<slug>/` falling through, an event with a key twice passing, an empty JSON body setting `ok`, the head timeout and two answerer paths untested, and the question of caching, answered by decision 19; Task 3's one named `Egress`; Task 4's false and stale sentences in the spec. The whole-branch review **requested changes**: its blocker, a key spelt in another case read by a case-insensitive upstream decoder (decision 6), and its minor findings (a slug not UTF-8, the filter's known gaps, written into §5.5, the spec's wording) were taken; its finding 2 is a product question ("After this plan"). Its **scoped re-confirmation** of what changed after the security review approved decision 19, the Task 2 fixes, the shared `Egress` and the Q1 ruling's record, and approved plan 8b-ii's obligations **after one amendment**: the answerer kept its client and credential for the stream's life, which the obligation forbids; taken (decision 14). A fresh opus reviewer's **scoped re-confirmation of those fixes approved them after amendments**, all taken: the fold ignores `_` and `-` too (F1, Go's `encoding/json/v2`); §5.5's list of the filter's gaps names a `tools/list` response spelt otherwise (F2: the refusal still enforces); and the product question's reason is that nothing mints a token before 8e (F3). It checked Go's folding rune by rune: only `ſ` and the Kelvin sign fold to ASCII letters that matter.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`.
- After every task the five checks pass: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p hennery --locked -- -D warnings`; `cargo test --workspace --locked`; `cargo run -p hennery-proto --bin gen -- --check`.
- **No new crate**; `Cargo.lock` changes by three lines in the gateway's stanza.
- **The gateway never depends on `hennery-sessions`** (`tests/boundary.rs`), and never reads a session table: session ids are opaque.
- **Every SQL statement names the owner**, and each new file with SQL is in the owner audit.
- **No test-only bypass in production code** (L7): test upstreams are on loopback, their connections marked `internal_network`.
- **No full upstream URL** in a log line, an error body or a `Debug` (L11); no token or credential in either (L8). Every log line names the connection.
- The proxy sends only through `hennery_kernel::egress`, never a client of its own, and never follows a redirect.
- Timing-dependent tests (the 5 s first-chunk bound, the freed permit polled for 5 s) pass with four copies of their binary in parallel. No test reads another process's state.
- Commits: Conventional Commits, gmail identity, unsigned. Push after every task; never push `main`.

## Review Focus

1. **A token out of scope** (unknown, revoked, superseded, another host's, a revoked host's), a connection out of scope (another hat's, unmounted, unknown), no or two `Authorization` headers.
   - Expected: one byte-identical 404; the upstream sees nothing.
   - Tests: `out_of_scope_is_one_404`; `scope_is_the_hat_s_connections_mounted_on_the_host`; `a_revoked_host_s_tokens_resolve_to_nothing`.
2. **What goes upstream.** The credential under its header with its prefix; the allowed headers; never the client's `Authorization`, cookie or `Origin`; the connection's URL without the client's query.
   - Tests: `a_request_goes_up_with_the_credential_and_the_allowed_headers_only`; `a_static_token_under_its_own_header_and_no_authorization_upstream`; `a_none_connection_sends_no_credential`.
3. **What comes down.** `Mcp-Session-Id`, one `Content-Type` of the gateway's, `nosniff` and `Cache-Control: no-store` (decision 19). Never `Set-Cookie` or `WWW-Authenticate`. Never a 401 or a 3xx. Only JSON or an event stream, uncompressed; a parameterised or doubled type cannot smuggle a stream past the filter.
   - Tests: the same first test; `an_upstream_401_is_502_upstream_auth_and_needs_auth`; `a_redirect_is_502_and_never_followed`; `only_json_and_event_streams_pass`; `the_answer_s_type_is_the_one_the_gateway_judged`.
4. **Streaming.** An upstream that sends one chunk and blocks.
   - Expected: the chunk arrives within 5 s; a buffering proxy fails the test, it does not hang.
   - Test: `the_first_chunk_arrives_before_the_upstream_finishes`; revert-probes `stream-json-unbuffered` and `stream-sse-unbuffered`.
5. **The allowlist.** `tools/call` outside it, alone or in a batch, never reaches the upstream. `tools/list` is filtered in JSON, a batch and SSE, with `[]` for nothing. A body with a key twice is refused.
   - Tests: `a_tools_call_outside_the_allowlist_never_reaches_the_upstream`; `tools_list_is_filtered_in_json_batches_and_event_streams`; `a_body_that_is_not_json_or_has_a_key_twice_is_400`; the `jsonrpc` unit tests.
6. **Capabilities.** `initialize` without `sampling`, `elicitation` and `roots`; a sampling request in a stream answered upstream and not passed on, also behind a byte-order mark, in data serde_json cannot read, or in an unended last event.
   - Tests: `initialize_goes_up_without_the_capabilities_not_forwarded`; `a_server_request_for_sampling_is_answered_and_not_passed_on`; `an_event_stream_fails_closed_on_what_it_cannot_read`.
7. **Limits and caps.** 413 past 4 MiB, declared or streamed; 408 for a body that stalls; 503 past the request and stream caps, and the place freed when a client goes; a `GET` holding no request permit; 502 past 8 MiB of `tools/list`; a stream ended past an 8 MiB event.
   - Tests: `a_body_over_4_mib_is_413`; `a_body_that_does_not_arrive_in_time_is_408`; `requests_past_the_cap_are_503`; `get_streams_are_forwarded_and_capped`; `a_get_stream_takes_no_request_permit`; `a_filtered_tools_list_over_8_mib_is_502`; `an_event_over_8_mib_ends_the_stream`.
8. **The egress allowance** from the stored flag only.
   - Test: `the_egress_allowance_is_the_connection_s_stored_flag`.
9. **Hygiene.** The session token, the static credential and a URL's path and query secrets never logged, at TRACE, across the success, 401, redirect, wrong type, cut body, cut stream, refused server request, unreachable and 404 paths; and never answered.
   - Tests: `no_token_credential_or_url_secret_is_logged_or_answered`, which also asserts that the capture holds the proxy's lines; `a_token_s_debug_shows_nothing_of_it`.
10. **The primitives inside the caller's transaction.** A rolled-back mint or revoke leaves nothing; a supersede, a host revoke and a hat purge.
    - Tests: `a_rolled_back_mint_leaves_no_token`; `the_next_mint_supersedes_the_session_s_token`; `a_host_revoke_revokes_that_host_s_tokens_only`; `a_hat_purge_takes_its_tokens_and_then_the_hat_can_go`.
11. **Outside the operator's routes.** A cross-site, cookie-less, non-JSON request to the collector's `/mcp/…` gets the proxy's 404, not 403 or 415.
    - Test: `the_collector_serves_the_mcp_proxy_outside_the_operator_s_routes`.

**Every outcome, its test and its revert-probe** (the fleet rule of 2026-10-02: each outcome a classifier can produce has a positive test of its own and a probe that makes that test fail):

| Classifier | Outcome | Test | Probe |
|---|---|---|---|
| Token → principal (`resolve`) | ok | `a_minted_token_resolves_to_its_session_host_and_hat` | `oc-resolve-ok` |
| | unknown | `out_of_scope_is_one_404` | `oc-resolve-unknown` |
| | revoked | `a_revoked_token_resolves_to_nothing_and_revoking_twice_is_fine` | `resolve-revoked`, `revoke` |
| | superseded | `the_next_mint_supersedes_the_session_s_token` | `mint-supersedes-hash` |
| | its host revoked | `a_revoked_host_s_tokens_resolve_to_nothing` | `resolve-host-revoked`, `revoke-host` |
| | not a token's shape | `out_of_scope_is_one_404` | none: the same 404 as unknown, and the check only spares a lookup |
| Principal → connection (`MountPolicy`) | ok | `scope_is_the_hat_s_connections_mounted_on_the_host` | `oc-scope-ok` |
| | another hat's | the same, and `out_of_scope_is_one_404` | `scope-hat` |
| | not mounted on the host | the same | `scope-mounted`, `scope-host` |
| | mounted on a revoked host | `out_of_scope_is_one_404` | `scope-host-revoked` |
| Request | another method, a deeper path, a slug not UTF-8 | `out_of_scope_is_one_404` | `t2r-head-404`, `t2r-method-fallback`, `t2r-trailing-slash-404`, `o7-deeper-404`, `wb-slug-utf8` |
| | busy, 503 | `requests_past_the_cap_are_503`, `get_streams_are_forwarded_and_capped` | `requests-limiter`, `streams-limiter` |
| | 413 | `a_body_over_4_mib_is_413`, `a_body_declared_over_4_mib_is_413_before_it_is_read` | `body-cap`, `oc-body-declared` |
| | 408 | `a_body_that_does_not_arrive_in_time_is_408` | `o3-body-timeout` |
| | 400 | `a_body_that_is_not_json_or_has_a_key_twice_is_400` | `duplicate-keys`, `wb-respelt-*` |
| | a `tools/call` outside the allowlist | `a_tools_call_outside_the_allowlist_never_reaches_the_upstream` | `tools-call-blocked` |
| | nothing to answer, 202 | the same | `oc-nothing-to-answer` |
| | no credential to send, 502 `upstream_auth` | `a_static_connection_without_a_token_is_502_and_sends_nothing` | `static-without-token` |
| | 500 `internal` | `a_connection_the_store_holds_damaged_is_500_internal` | `oc-internal` |
| Upstream answer | 2xx, streamed | `the_first_chunk_arrives_before_the_upstream_finishes` | `stream-json-unbuffered`, `stream-sse-unbuffered` |
| | 401 → 502 `upstream_auth` | `an_upstream_401_is_502_upstream_auth_and_needs_auth` | `401-held`, `mark-needs-auth-call` |
| | another status, passed on | `live_traffic_sets_ok_on_a_2xx_only` | `oc-status-passed-through` |
| | 3xx → 502 `upstream_redirect` | `a_redirect_is_502_and_never_followed` | `redirect` |
| | refused content type → 502 `upstream_content_type` | `only_json_and_event_streams_pass`, `the_answer_s_type_is_the_one_the_gateway_judged` | `content-type-other`, `content-type-none-with-body`, `content-encoding`, `b1-two-types` |
| | not reached → 502 `upstream_unreachable` | `an_upstream_that_never_answers_is_502_after_the_head_timeout` | `oc-unreachable`, `t2r-head-timeout` |
| | filtered `tools/list` over 8 MiB → 502 `upstream_too_large` | `a_filtered_tools_list_over_8_mib_is_502` | `filtered-cap` |
| | filtered `tools/list` that does not parse → 502 `upstream_invalid` | `a_filtered_tools_list_that_does_not_parse_is_502` | `oc-upstream-invalid` |
| | a refused server-to-client request | `a_server_request_for_sampling_is_answered_and_not_passed_on` | `answer-server-request`, `drop-server-request` |
| | an event the gateway cannot read | `an_event_stream_fails_closed_on_what_it_cannot_read` | `b2-*`, `bom-line`, `t2r-sse-duplicate-keys`, `wb-respelt-down` |
| Egress allowance (`egress_client`) | internal network, marked | `the_egress_allowance_is_the_connection_s_stored_flag` | `oc-egress-internal-arm` |
| | public only, unmarked | the same, and `an_unmarked_connection_is_refused_plain_http_to_a_lan_address` | `egress-from-stored-flag`, `egress-plain-http-lan` |

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `crates/hennery-gateway/src/schema.rs` | The `gw_session_tokens` migration | 1 |
| `crates/hennery-gateway/src/tokens.rs` | Token shape, `SessionToken`, the transaction primitives | 1 |
| `crates/hennery-gateway/src/scope.rs` | `Principal`, `ClientIdentity`, `MountPolicy`, `ScopedConnection`, `ProxyStore` | 1 |
| `crates/hennery-gateway/src/lib.rs` | The modules | 1, 2 |
| `crates/hennery-gateway/tests/support/mod.rs` | `World`: a `hennery.db` with hosts, hats, connections and tokens | 1, 2 |
| `crates/hennery-gateway/tests/tokens.rs` | The primitives and scope | 1 |
| `crates/hennery-gateway/tests/store.rs` | 8a's version pin: 2 | 1 |
| `crates/hennery-testkit/tests/owner_filter.rs` | `tokens.rs` and `scope.rs` audited | 1 |
| `crates/hennery-gateway/src/jsonrpc.rs` | The allowlist, capabilities, SSE events | 2 |
| `crates/hennery-gateway/src/proxy.rs` | The route, `Limits`, `ProxyState` | 2 |
| `crates/hennery-gateway/Cargo.toml`, `Cargo.lock` | `futures`, `reqwest`, `serde`, `tokio` | 2 |
| `crates/hennery-gateway/tests/support/upstream.rs` | The fake MCP upstream; `Harness`, the proxy over loopback | 2 |
| `crates/hennery-gateway/tests/proxy.rs` | The proxy | 2 |
| `crates/hennery-gateway/tests/proxy_log.rs` | Hygiene, in its own binary | 2 |
| `crates/hennery-gateway/tests/proxy_egress.rs` | Plain `http` to a LAN address refused unmarked (8b-ii), in its own binary | 2 |
| `crates/hennery/src/main.rs` | The proxy on the collector's one `Egress`, its router | 3 |
| `crates/hennery/tests/cli.rs` | The route through the collector | 3 |
| `docs/specs/2026-09-26-mcp-gateway-design.md` | §3.1, §5, §7 written back | 4 |
| `docs/specs/2026-09-26-kernel-design.md`, `docs/specs/2026-09-25-hennery-architecture-design.md` | §7.1's egress now has a caller; §10.1's first-chunk rule for event streams | 4 |

**Reading the steps:** as in plan 5a. "Create `path`:" makes a new file with the block. "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement. "Run:" lines only check, except `cargo run -p hennery-proto --bin gen`, which regenerates the protocol files.

---

### Task 1: Session tokens, `ClientIdentity` and `MountPolicy`

**Files:**
- Create: `crates/hennery-gateway/src/tokens.rs`, `crates/hennery-gateway/src/scope.rs`, `crates/hennery-gateway/tests/support/mod.rs`, `crates/hennery-gateway/tests/tokens.rs`
- Modify: `crates/hennery-gateway/src/schema.rs`, `crates/hennery-gateway/src/lib.rs`, `crates/hennery-gateway/tests/store.rs`, `crates/hennery-testkit/tests/owner_filter.rs`

**Anchors:** 8a's `MIGRATIONS` ends with `CREATE INDEX gw_mounts_by_host`; `GatewayStore::owner_id`, `purge_hat`, `set_static_credential` and `replace_mounts` (`store.rs`); `db::open`, `db::kernel_owner`, `db::migrate_component` (kernel `db.rs`); `secret::random_bytes`, `secret::sha256_hex`; the owner audit's `SOURCES` ends with the gateway's `store.rs`.

- [ ] **Step 1: Write the failing tests**

In `crates/hennery-gateway/tests/store.rs`, replace:

```rust
        .unwrap();
    assert_eq!(version, 1);
}
```

with:

```rust
        .unwrap();
    // Plan 8a's tables, then plan 8d's session tokens.
    assert_eq!(version, 2);
}
```

Create `crates/hennery-gateway/tests/support/mod.rs`:

```rust
//! A fresh `hennery.db` with the gateway's stores, hosts, hats and
//! connections, and session tokens minted and revoked through the
//! primitives plan 8e will call, each in a transaction of its own as the
//! sessions store's would be (plan 8d).

#![allow(dead_code)]

use ed25519_dalek::SigningKey;
use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{Change, CredKind, CredentialChange, NewConnection};
use hennery_gateway::scope::ProxyStore;
use hennery_gateway::store::GatewayStore;
use hennery_gateway::tokens;
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::secret::unix_now;
use std::path::PathBuf;
use std::sync::Arc;

pub struct World {
    pub dir: tempfile::TempDir,
    pub db: PathBuf,
    pub store: Arc<GatewayStore>,
    pub proxy_store: Arc<ProxyStore>,
    pub key: Arc<MasterKey>,
    pub hosts: Hosts,
}

impl World {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let store = Arc::new(GatewayStore::open(&db).unwrap());
        let proxy_store = Arc::new(ProxyStore::open(&db).unwrap());
        Self {
            dir,
            db,
            store,
            proxy_store,
            key: Arc::new(MasterKey::from_bytes([7; 32])),
            hosts,
        }
    }

    pub fn hat(&self) -> String {
        self.hosts.default_hat_for_new_hosts().unwrap()
    }

    pub fn other_hat(&self) -> String {
        match self.hosts.create_hat("Work", None, unix_now()).unwrap() {
            HatChange::Done(hat) => hat.id,
            other => panic!("{other:?}"),
        }
    }

    pub fn host(&self, id: &str, seed: u8) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let enrollment = Enrollment {
            public_key: hex::encode(key.verifying_key().as_bytes()),
            name: format!("host {seed}"),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        self.hosts.register(id, &enrollment, unix_now()).unwrap();
    }

    /// A connection to `url` in `hat`, on the internal network (lane L7:
    /// test upstreams are on loopback).
    pub fn connection_in(
        &self,
        slug: &str,
        url: &str,
        kind: CredKind,
        hat: &str,
        allowlist: Option<&[&str]>,
    ) -> String {
        self.connection_with(NewConnection {
            slug: slug.into(),
            label: format!("Label {slug}"),
            url: url.into(),
            hat_id: hat.into(),
            cred_kind: kind,
            static_header: None,
            static_prefix: None,
            tool_allowlist: allowlist.map(|tools| tools.iter().map(|t| t.to_string()).collect()),
            internal_network: true,
        })
    }

    pub fn connection_with(&self, new: NewConnection) -> String {
        match self.store.create(&new, unix_now()).unwrap() {
            Change::Done(record) => record.id,
            other => panic!("{other:?}"),
        }
    }

    pub fn connection(&self, slug: &str, url: &str, kind: CredKind) -> String {
        self.connection_in(slug, url, kind, &self.hat(), None)
    }

    pub fn mount(&self, id: &str, hosts: &[&str]) {
        let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
        assert!(matches!(
            self.store.replace_mounts(id, &hosts).unwrap(),
            Change::Done(_)
        ));
    }

    pub fn set_token(&self, id: &str, token: &str) {
        assert_eq!(
            self.store
                .set_static_credential(id, token, &self.key, unix_now())
                .unwrap(),
            CredentialChange::Done
        );
    }

    /// A raw connection to `hennery.db`, as the sessions store's would be.
    pub fn raw(&self) -> rusqlite::Connection {
        hennery_kernel::db::open(&self.db).unwrap()
    }

    pub fn mint(&self, session: &str, host: &str, hat: &str) -> String {
        let mut conn = self.raw();
        let tx = conn.transaction().unwrap();
        let token = tokens::mint_in(&tx, self.store.owner_id(), session, host, hat, unix_now()).unwrap();
        tx.commit().unwrap();
        token.expose().to_string()
    }

    pub fn revoke(&self, session: &str) -> bool {
        let mut conn = self.raw();
        let tx = conn.transaction().unwrap();
        let revoked = tokens::revoke_in(&tx, self.store.owner_id(), session, unix_now()).unwrap();
        tx.commit().unwrap();
        revoked
    }

    pub fn revoke_host_tokens(&self, host: &str) -> usize {
        let mut conn = self.raw();
        let tx = conn.transaction().unwrap();
        let revoked = tokens::revoke_host_in(&tx, self.store.owner_id(), host, unix_now()).unwrap();
        tx.commit().unwrap();
        revoked
    }

    pub fn status(&self, id: &str) -> String {
        self.store.connection(id).unwrap().unwrap().status
    }
}
```

Create `crates/hennery-gateway/tests/tokens.rs`:

```rust
//! Session tokens and scope (gateway spec §3.1, umbrella §10.2; lane L1):
//! the primitives plan 8e calls inside the sessions store's transactions,
//! and what `ProxyStore` resolves from them. Driven on `hennery.db`
//! directly, as the sessions store would.

mod support;

use hennery_gateway::model::CredKind;
use hennery_gateway::scope::{ClientIdentity, LAST_USED_EVERY, MountPolicy, Principal, PrincipalKind};
use hennery_gateway::tokens::{self, SESSION_TOKEN_PREFIX, is_session_token};
use hennery_kernel::secret::unix_now;
use rusqlite::OptionalExtension;
use support::World;

fn principal(session: &str, host: &str, hat: &str) -> Principal {
    Principal {
        hat_id: hat.into(),
        kind: PrincipalKind::Session {
            session_id: session.into(),
            host_id: host.into(),
        },
    }
}

#[test]
fn a_minted_token_resolves_to_its_session_host_and_hat() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let token = h.mint("s1", "host-a", &hat);
    assert!(token.starts_with(SESSION_TOKEN_PREFIX), "{token}");
    assert!(is_session_token(&token));
    assert_eq!(
        h.proxy_store.resolve(&token, unix_now()).unwrap(),
        Some(principal("s1", "host-a", &hat))
    );
    // Only its hash is stored.
    let stored: String = h
        .raw()
        .query_row(
            "SELECT token_hash FROM gw_session_tokens WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, hennery_kernel::secret::sha256_hex(token.as_bytes()));
    let db = std::fs::read(&h.db).unwrap();
    let wal = std::fs::read(h.db.with_extension("db-wal")).unwrap_or_default();
    for bytes in [db, wal] {
        assert!(
            !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "the token is stored in clear"
        );
    }
    // Two mints never give the same token.
    let other = h.mint("s2", "host-a", &hat);
    assert_ne!(other, token);
}

#[test]
fn the_next_mint_supersedes_the_session_s_token() {
    let h = World::new();
    h.host("host-a", 1);
    h.host("host-b", 2);
    let hat = h.hat();
    let work = h.other_hat();
    let first = h.mint("s1", "host-a", &hat);
    // A resume, in another hat on another host (a re-assignment).
    let second = h.mint("s1", "host-b", &work);
    assert_eq!(h.proxy_store.resolve(&first, unix_now()).unwrap(), None, "superseded");
    assert_eq!(
        h.proxy_store.resolve(&second, unix_now()).unwrap(),
        Some(principal("s1", "host-b", &work))
    );
    // A mint after a revoke gives a live token again, and only the new one.
    assert!(h.revoke("s1"));
    let third = h.mint("s1", "host-a", &hat);
    assert_eq!(h.proxy_store.resolve(&second, unix_now()).unwrap(), None);
    assert_eq!(
        h.proxy_store.resolve(&third, unix_now()).unwrap(),
        Some(principal("s1", "host-a", &hat))
    );
}

#[test]
fn a_revoked_token_resolves_to_nothing_and_revoking_twice_is_fine() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let token = h.mint("s1", "host-a", &hat);
    let kept = h.mint("s2", "host-a", &hat);
    assert!(h.revoke("s1"));
    assert!(!h.revoke("s1"), "already revoked");
    assert!(!h.revoke("never-minted"));
    assert_eq!(h.proxy_store.resolve(&token, unix_now()).unwrap(), None);
    assert!(h.proxy_store.resolve(&kept, unix_now()).unwrap().is_some());
}

#[test]
fn a_host_revoke_revokes_that_host_s_tokens_only() {
    let h = World::new();
    h.host("host-a", 1);
    h.host("host-b", 2);
    let hat = h.hat();
    let a1 = h.mint("s1", "host-a", &hat);
    let a2 = h.mint("s2", "host-a", &hat);
    let b = h.mint("s3", "host-b", &hat);
    assert_eq!(h.revoke_host_tokens("host-a"), 2);
    assert_eq!(h.revoke_host_tokens("host-a"), 0);
    for token in [&a1, &a2] {
        assert_eq!(h.proxy_store.resolve(token, unix_now()).unwrap(), None);
    }
    assert!(h.proxy_store.resolve(&b, unix_now()).unwrap().is_some());
}

/// Even without the gateway's revoke, a revoked host's tokens resolve to
/// nothing: the resolve reads the host (a missed revoke site in plan 8e
/// stays closed).
#[test]
fn a_revoked_host_s_tokens_resolve_to_nothing() {
    let h = World::new();
    h.host("host-a", 1);
    let token = h.mint("s1", "host-a", &h.hat());
    h.hosts.revoke("host-a", unix_now()).unwrap();
    assert_eq!(h.proxy_store.resolve(&token, unix_now()).unwrap(), None);
}

/// Lane L1: a mint is part of the caller's transaction, so a transition
/// that rolls back leaves no token behind.
#[test]
fn a_rolled_back_mint_leaves_no_token() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    let token = tokens::mint_in(&tx, h.store.owner_id(), "s1", "host-a", &hat, unix_now()).unwrap();
    tx.rollback().unwrap();
    assert_eq!(h.proxy_store.resolve(token.expose(), unix_now()).unwrap(), None);
    let row: Option<String> = h
        .raw()
        .query_row("SELECT session_id FROM gw_session_tokens", [], |r| r.get(0))
        .optional()
        .unwrap();
    assert_eq!(row, None);
    // A revoke rolled back leaves the token live.
    let live = h.mint("s2", "host-a", &hat);
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    assert!(tokens::revoke_in(&tx, h.store.owner_id(), "s2", unix_now()).unwrap());
    tx.rollback().unwrap();
    assert!(h.proxy_store.resolve(&live, unix_now()).unwrap().is_some());
}

/// A host or hat that is not the owner's fails the mint (their foreign
/// keys), and with it the caller's transition.
#[test]
fn a_mint_for_an_unknown_host_or_hat_fails() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    assert!(tokens::mint_in(&tx, h.store.owner_id(), "s1", "nowhere", &hat, unix_now()).is_err());
    assert!(tokens::mint_in(&tx, h.store.owner_id(), "s1", "host-a", "hat-none", unix_now()).is_err());
}

#[test]
fn malformed_and_unknown_tokens_resolve_to_nothing() {
    let h = World::new();
    h.host("host-a", 1);
    let token = h.mint("s1", "host-a", &h.hat());
    let unknown = format!("{SESSION_TOKEN_PREFIX}{}", "0".repeat(64));
    for bad in [
        "",
        "Bearer",
        unknown.as_str(),
        &token[..token.len() - 1],
        &token.to_uppercase(),
        &format!("{token}0"),
    ] {
        assert_eq!(h.proxy_store.resolve(bad, unix_now()).unwrap(), None, "{bad}");
    }
}

/// Plan 8d decision 12: `last_used_at` is written on use, at most once a
/// minute.
#[test]
fn use_is_recorded_at_most_once_a_minute() {
    let h = World::new();
    h.host("host-a", 1);
    let token = h.mint("s1", "host-a", &h.hat());
    let used = |h: &World| -> Option<i64> {
        h.raw()
            .query_row(
                "SELECT last_used_at FROM gw_session_tokens WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(used(&h), None);
    let t0 = 1_000_000;
    h.proxy_store.resolve(&token, t0).unwrap().unwrap();
    assert_eq!(used(&h), Some(t0));
    h.proxy_store
        .resolve(&token, t0 + LAST_USED_EVERY - 1)
        .unwrap()
        .unwrap();
    assert_eq!(used(&h), Some(t0));
    h.proxy_store.resolve(&token, t0 + LAST_USED_EVERY).unwrap().unwrap();
    assert_eq!(used(&h), Some(t0 + LAST_USED_EVERY));
    // A new token starts unused, and its first use is recorded.
    let next = h.mint("s1", "host-a", &h.hat());
    assert_eq!(used(&h), None);
    h.proxy_store.resolve(&next, t0 + LAST_USED_EVERY + 1).unwrap().unwrap();
    assert_eq!(used(&h), Some(t0 + LAST_USED_EVERY + 1));
}

/// Decision 2: a mint never takes over another owner's row under the same
/// session id; it fails, and the row stays as it was. The host and hat
/// foreign keys would refuse the takeover too; the upsert's own guard
/// refuses it first, by name.
#[test]
fn a_mint_never_takes_over_another_owner_s_token() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let conn = h.raw();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO owners(id, created_at) VALUES ('other', 0);
         INSERT INTO gw_session_tokens(session_id, owner_id, host_id, hat_id, token_hash, created_at)
         VALUES ('s1', 'other', 'x', 'y', 'deadbeef', 0);",
    )
    .unwrap();
    drop(conn);
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    let err = tokens::mint_in(&tx, h.store.owner_id(), "s1", "host-a", &hat, unix_now()).unwrap_err();
    assert!(err.to_string().contains("another owner's"), "{err:#}");
    drop(tx);
    let row: (String, String) = h
        .raw()
        .query_row(
            "SELECT owner_id, token_hash FROM gw_session_tokens WHERE session_id = 's1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(row, ("other".to_string(), "deadbeef".to_string()));
}

#[test]
fn scope_is_the_hat_s_connections_mounted_on_the_host() {
    let h = World::new();
    h.host("host-a", 1);
    h.host("host-b", 2);
    let hat = h.hat();
    let work = h.other_hat();
    let mine = h.connection("mine", "http://127.0.0.1:9/mcp", CredKind::None);
    let theirs = h.connection_in("theirs", "http://127.0.0.1:9/mcp", CredKind::None, &work, None);
    let elsewhere = h.connection("elsewhere", "http://127.0.0.1:9/mcp", CredKind::None);
    h.mount(&mine, &["host-a"]);
    h.mount(&theirs, &["host-a"]);
    h.mount(&elsewhere, &["host-b"]);
    let me = principal("s1", "host-a", &hat);
    let found = h.proxy_store.connection(&me, "mine").unwrap().unwrap();
    assert_eq!(found.id, mine);
    assert_eq!(found.slug, "mine");
    assert_eq!(found.cred_kind, CredKind::None);
    assert!(found.internal_network);
    // Another hat's, mounted here: out of scope. Mounted elsewhere: out.
    // No such slug: out.
    for slug in ["theirs", "elsewhere", "nothing"] {
        assert_eq!(h.proxy_store.connection(&me, slug).unwrap(), None, "{slug}");
    }
    // The other hat's principal on this host reaches its own.
    let them = principal("s2", "host-a", &work);
    assert_eq!(h.proxy_store.connection(&them, "theirs").unwrap().unwrap().id, theirs);
    assert_eq!(h.proxy_store.connection(&them, "mine").unwrap(), None);
    // Unmounted now: out at once.
    h.mount(&mine, &[]);
    assert_eq!(h.proxy_store.connection(&me, "mine").unwrap(), None);
    // A revoked host's mounts are out too.
    h.mount(&mine, &["host-a"]);
    h.hosts.revoke("host-a", unix_now()).unwrap();
    assert_eq!(h.proxy_store.connection(&me, "mine").unwrap(), None);
}

/// Lane L6 and gateway spec §2: a hat's tokens go with its purge, revoked
/// ones too, so the hat row can be deleted once the gateway's purge has
/// run; without it, the tokens' foreign key keeps the hat.
#[test]
fn a_hat_purge_takes_its_tokens_and_then_the_hat_can_go() {
    let h = World::new();
    h.host("host-a", 1);
    let hat = h.hat();
    let work = h.other_hat();
    let live = h.mint("s1", "host-a", &work);
    h.mint("s2", "host-a", &work);
    h.revoke("s2");
    let kept = h.mint("s3", "host-a", &hat);
    let delete_hat = |h: &World| {
        h.raw().execute(
            "DELETE FROM hats WHERE id = ?1 AND owner_id = ?2",
            [&work, h.store.owner_id()],
        )
    };
    h.store.purge_hat(&work).unwrap();
    assert!(delete_hat(&h).is_err(), "the hat went with tokens left");
    let mut conn = h.raw();
    let tx = conn.transaction().unwrap();
    assert_eq!(tokens::purge_hat_in(&tx, h.store.owner_id(), &work).unwrap(), 2);
    assert_eq!(tokens::purge_hat_in(&tx, h.store.owner_id(), &work).unwrap(), 0);
    tx.commit().unwrap();
    assert_eq!(h.proxy_store.resolve(&live, unix_now()).unwrap(), None);
    assert!(h.proxy_store.resolve(&kept, unix_now()).unwrap().is_some());
    assert_eq!(delete_hat(&h).unwrap(), 1);
}

/// Gateway spec §7: live traffic sets `ok`, and a 401 `needs_auth`, only for
/// the connection as it was when the request went: not after its URL moved,
/// nor for a static connection whose token was deleted meanwhile.
#[test]
fn live_status_is_only_for_the_connection_as_it_was() {
    let h = World::new();
    let url = "http://127.0.0.1:9/mcp";
    let id = h.connection("linear", url, CredKind::Static);
    assert!(!h.proxy_store.mark_ok(&id, url, 1).unwrap(), "no token: not ok");
    h.set_token(&id, "tok");
    assert!(!h.proxy_store.mark_ok(&id, "http://127.0.0.1:9/other", 1).unwrap());
    assert!(
        !h.proxy_store
            .mark_needs_auth(&id, "http://127.0.0.1:9/other", 1)
            .unwrap()
    );
    assert_eq!(h.status(&id), "not_connected");
    assert!(h.proxy_store.mark_needs_auth(&id, url, 2).unwrap());
    assert!(!h.proxy_store.mark_needs_auth(&id, url, 3).unwrap(), "already");
    assert_eq!(h.status(&id), "needs_auth");
    assert!(h.proxy_store.mark_ok(&id, url, 4).unwrap());
    assert!(!h.proxy_store.mark_ok(&id, url, 5).unwrap(), "already");
    assert_eq!(h.status(&id), "ok");
    let none = h.connection("public", url, CredKind::None);
    assert!(h.proxy_store.mark_ok(&none, url, 1).unwrap());
}
```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
        GATEWAY_STATEMENTS,
```

with:

```rust
        GATEWAY_STATEMENTS,
    ),
    // Plan 8d: session tokens, and the proxy's reads and writes.
    (
        "hennery-gateway/src/tokens.rs",
        include_str!("../../hennery-gateway/src/tokens.rs"),
        4,
    ),
    (
        "hennery-gateway/src/scope.rs",
        include_str!("../../hennery-gateway/src/scope.rs"),
        5,
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-gateway --locked --test tokens`
Expected: FAIL to compile: unresolved imports `hennery_gateway::scope`, `hennery_gateway::tokens`.

- [ ] **Step 3: Commit the tests**

```bash
git add crates/hennery-gateway/tests crates/hennery-testkit/tests/owner_filter.rs
git commit -m "test(gateway): session tokens, minted and revoked in the caller's transaction, and their scope"
```

- [ ] **Step 4: The table, the primitives and the scope**

In `crates/hennery-gateway/src/lib.rs`, replace:

```rust
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod api;
pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod store;
```

with:

```rust
//! the hosts they are mounted on, session tokens, and the proxy that
//! forwards a token's requests to its connections. Depends on
//! `hennery-kernel` and `hennery-proto`, never on `hennery-sessions`
//! (umbrella §9).

pub mod api;
pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod scope;
pub mod store;
pub mod tokens;
```

In `crates/hennery-gateway/src/schema.rs`, replace:

```rust
//! and the sessions store's. Plan 8a makes the first three; later plans add
//! the session tokens, standalone clients, OAuth clients and stdio servers.
```

with:

```rust
//! and the sessions store's. Plan 8a makes the first three, plan 8d the
//! session tokens; later plans add standalone clients, OAuth clients and
//! stdio servers.
```

In `crates/hennery-gateway/src/schema.rs`, replace:

```rust
pub(crate) const MIGRATIONS: &[&str] = &["
```

with:

```rust
pub(crate) const MIGRATIONS: &[&str] = &[
    "
```

In `crates/hennery-gateway/src/schema.rs`, replace:

```rust
    "];
```

with:

```rust
    ",
    // Plan 8d: one token per session (gateway spec §3.1), minted and
    // revoked inside the sessions store's own transactions (lane L1). The
    // session id is an opaque value with no foreign key: the gateway never
    // reads session tables (lane L6). The host and the hat are the
    // owner's, as everywhere else; a hat's tokens go with its purge
    // (`tokens::purge_hat_in`) before the hat row can be deleted. Only the
    // token's SHA-256 is stored.
    "
    CREATE TABLE gw_session_tokens (
        session_id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        host_id TEXT NOT NULL,
        hat_id TEXT NOT NULL,
        token_hash TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL,
        last_used_at INTEGER,
        revoked_at INTEGER,
        FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id),
        FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id));
    CREATE INDEX gw_session_tokens_by_host ON gw_session_tokens(owner_id, host_id);
    CREATE INDEX gw_session_tokens_by_hat ON gw_session_tokens(owner_id, hat_id);
    ",
];
```

Create `crates/hennery-gateway/src/scope.rs`:

```rust
//! Who presents a token, and what it may reach (gateway spec §3.1,
//! umbrella §10.2): `ClientIdentity` turns a token into a `Principal`,
//! `MountPolicy` decides whether a connection is in that principal's
//! scope. Both are checked at every request, from the token and the mounts
//! as they are then, never from anything the request claims.
//!
//! Full mode: `ProxyStore` implements both. A session token resolves to
//! "session S on host X, hat H"; its connections are those of hat H
//! mounted on host X, a host that is not revoked. Standalone mode (plan
//! 8g) adds a client principal, resolved from `gw_clients`, whose
//! connections are its pins.
//!
//! Every SQL statement of this file names the owner, and the owner audit
//! reads it (`hennery-testkit/tests/owner_filter.rs`).

use crate::model::{CredKind, url_for_logs};
use crate::schema::{COMPONENT, MIGRATIONS};
use crate::tokens::{is_session_token, token_hash};
use anyhow::{Context, Result, anyhow};
use hennery_kernel::db;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// How stale `last_used_at` may get before a request writes it again, in
/// seconds (plan 8d decision 12): one write a minute per session at most,
/// not one per request.
pub const LAST_USED_EVERY: i64 = 60;

/// Who presented a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// The hat whose connections it may reach, and no other's.
    pub hat_id: String,
    pub kind: PrincipalKind,
}

/// The kinds of principal (gateway spec §3.1). Plan 8g adds the standalone
/// client, scoped by its pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrincipalKind {
    /// A hennery session: its connections are those of its hat mounted on
    /// its host.
    Session { session_id: String, host_id: String },
}

/// Token → principal (umbrella §10.2).
pub trait ClientIdentity: Send + Sync + 'static {
    /// The principal `token` names now, or `None`: unknown, revoked,
    /// superseded, or of a revoked host. Each is the same 404 to the client.
    fn resolve(&self, token: &str, now: i64) -> Result<Option<Principal>>;
}

/// Principal → connections (umbrella §10.2).
pub trait MountPolicy: Send + Sync + 'static {
    /// The connection with `slug` if it is in `principal`'s scope now;
    /// `None` for one that is not, or does not exist (the same 404).
    fn connection(&self, principal: &Principal, slug: &str) -> Result<Option<ScopedConnection>>;
}

/// A connection as the proxy needs it, read in scope. Where a `static`
/// connection's token goes is read again with the token itself
/// (`GatewayStore::static_credential`, the review's R2 of plan 8a).
#[derive(Clone, PartialEq, Eq)]
pub struct ScopedConnection {
    pub id: String,
    pub slug: String,
    pub label: String,
    pub url: String,
    pub cred_kind: CredKind,
    pub internal_network: bool,
    /// `None`: every tool.
    pub tool_allowlist: Option<Vec<String>>,
    /// `not_connected`, `ok`, `needs_auth` or `error`.
    pub status: String,
}

// `Debug` by hand: the URL shows only its origin (lane L11).
impl std::fmt::Debug for ScopedConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedConnection")
            .field("id", &self.id)
            .field("slug", &self.slug)
            .field("label", &self.label)
            .field("url", &url_for_logs(&self.url))
            .field("cred_kind", &self.cred_kind)
            .field("internal_network", &self.internal_network)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("status", &self.status)
            .finish()
    }
}

/// The proxy's reads and writes of `hennery.db`, on a connection of its
/// own: session tokens resolved, connections in scope, and what live
/// traffic says of a connection's status (gateway spec §7).
pub struct ProxyStore {
    conn: Mutex<Connection>,
    owner: String,
}

impl ProxyStore {
    /// Open on `hennery.db`, migrating the kernel's tables and the
    /// gateway's first, as `GatewayStore::open` does.
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = db::open(path)?;
        let owner = db::kernel_owner(&mut conn)?;
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self {
            conn: Mutex::new(conn),
            owner,
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("proxy store lock")
    }

    pub fn owner_id(&self) -> &str {
        &self.owner
    }

    /// Live traffic got a 2xx through `id` with its URL `url` (gateway spec
    /// §7): its status becomes `ok` if it was anything else. Not if the
    /// connection was changed meanwhile, to another URL or to a `static`
    /// kind without a credential: the answer was about what it was. True if
    /// the status changed.
    pub fn mark_ok(&self, id: &str, url: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE gw_connections SET status = 'ok', status_note = NULL, status_at = ?4
             WHERE id = ?1 AND owner_id = ?2 AND url = ?3 AND status != 'ok'
                 AND (cred_kind = 'none'
                      OR EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = ?1 AND k.owner_id = ?2))",
            params![id, self.owner, url, now],
        )?;
        Ok(changed == 1)
    }

    /// The upstream at `url` refused `id`'s credential, or its lack of one
    /// (401; gateway spec §5.4, §7): `needs_auth`, unless the connection
    /// was changed to another URL meanwhile. True if the status changed.
    pub fn mark_needs_auth(&self, id: &str, url: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE gw_connections SET status = 'needs_auth', status_note = ?4, status_at = ?5
             WHERE id = ?1 AND owner_id = ?2 AND url = ?3 AND status != 'needs_auth'",
            params![id, self.owner, url, "the upstream refused the credential (401)", now],
        )?;
        Ok(changed == 1)
    }
}

impl ClientIdentity for ProxyStore {
    fn resolve(&self, token: &str, now: i64) -> Result<Option<Principal>> {
        if !is_session_token(token) {
            return Ok(None);
        }
        let hash = token_hash(token);
        let conn = self.conn();
        let row: Option<(String, String, String, Option<i64>)> = conn
            .query_row(
                "SELECT t.session_id, t.host_id, t.hat_id, t.last_used_at
                 FROM gw_session_tokens t JOIN hosts h ON h.id = t.host_id AND h.owner_id = ?1
                 WHERE t.owner_id = ?1 AND t.token_hash = ?2 AND t.revoked_at IS NULL AND h.revoked_at IS NULL",
                params![self.owner, hash],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((session_id, host_id, hat_id, last_used_at)) = row else {
            return Ok(None);
        };
        if last_used_at.is_none_or(|at| now - at >= LAST_USED_EVERY) {
            // `token_hash` too: only the row just read is stamped, never one
            // a mint superseded in between.
            conn.execute(
                "UPDATE gw_session_tokens SET last_used_at = ?3
                 WHERE session_id = ?1 AND owner_id = ?2 AND token_hash = ?4",
                params![session_id, self.owner, now, hash],
            )?;
        }
        Ok(Some(Principal {
            hat_id,
            kind: PrincipalKind::Session { session_id, host_id },
        }))
    }
}

impl MountPolicy for ProxyStore {
    fn connection(&self, principal: &Principal, slug: &str) -> Result<Option<ScopedConnection>> {
        let PrincipalKind::Session { host_id, .. } = &principal.kind;
        type Row = (String, String, String, String, bool, Option<String>, String);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT c.id, c.label, c.url, c.cred_kind, c.internal_network, c.tool_allowlist, c.status
                 FROM gw_connections c
                 WHERE c.owner_id = ?1 AND c.slug = ?2 AND c.hat_id = ?3
                     AND EXISTS (SELECT 1 FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
                                 WHERE m.connection_id = c.id AND m.owner_id = ?1 AND m.host_id = ?4
                                     AND h.revoked_at IS NULL)",
                params![self.owner, slug, principal.hat_id, host_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, label, url, kind, internal_network, allowlist, status)) = row else {
            return Ok(None);
        };
        Ok(Some(ScopedConnection {
            cred_kind: CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?,
            tool_allowlist: allowlist
                .map(|json| serde_json::from_str(&json))
                .transpose()
                .context("a stored tool allowlist")?,
            id,
            slug: slug.to_string(),
            label,
            url,
            internal_network,
            status,
        }))
    }
}
```

Create `crates/hennery-gateway/src/tokens.rs`:

```rust
//! Session tokens (gateway spec §3.1): one per session, minted at every
//! start and resume, revoked on park, close, adapter exit and host revoke,
//! and superseded by the next mint for the same session.
//!
//! The functions here take the caller's `rusqlite::Transaction` (lane L1):
//! the sessions store mints inside the transition that starts or resumes a
//! session, before it commits, so a failed mint rolls the transition back,
//! and revokes inside the transition that ends one. They touch only
//! `gw_session_tokens`, and every statement names the owner (lane L6).
//!
//! A token is `hnry_session_` and 64 lowercase hexadecimal digits: 32
//! random bytes behind a prefix a secret scanner can match (plan 8d
//! decision 1). Only its SHA-256 is stored; the plaintext exists in the
//! `SessionToken` the mint returns, and then only in the frame that carries
//! it to the session's host.

use anyhow::Result;
use hennery_kernel::secret::{random_bytes, sha256_hex};
use rusqlite::{Transaction, params};
use zeroize::Zeroizing;

/// What every session token starts with (plan 8d decision 1).
pub const SESSION_TOKEN_PREFIX: &str = "hnry_session_";

/// The random part's length, in hexadecimal digits (32 bytes).
const RANDOM_HEX: usize = 64;

/// A freshly minted session token. The plaintext is wiped from memory when
/// dropped and never shown by `Debug`.
#[derive(Clone)]
pub struct SessionToken(Zeroizing<String>);

impl SessionToken {
    /// The token, for the `Authorization: Bearer <token>` header of the
    /// session's `mcp_servers` entries (gateway spec §3.2) and nothing else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken(<redacted>)")
    }
}

/// Whether `token` has a session token's shape: the prefix, then exactly
/// 64 lowercase hexadecimal digits. Anything else is not looked up.
pub fn is_session_token(token: &str) -> bool {
    token
        .strip_prefix(SESSION_TOKEN_PREFIX)
        .is_some_and(|rest| rest.len() == RANDOM_HEX && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// How a token is stored and looked up: its SHA-256, in hexadecimal.
pub(crate) fn token_hash(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// Mint the token of `session_id` on `host_id` in `hat_id`, inside the
/// caller's transaction. It replaces the session's previous token, revoked
/// or not, which no longer resolves (superseded). The host and the hat
/// must be the owner's (their foreign keys), or the mint fails and the
/// caller's transaction should roll back.
pub fn mint_in(
    tx: &Transaction<'_>,
    owner_id: &str,
    session_id: &str,
    host_id: &str,
    hat_id: &str,
    now: i64,
) -> Result<SessionToken> {
    let random = Zeroizing::new(hex::encode(Zeroizing::new(random_bytes::<32>()).as_slice()));
    // Built at its final size, so no grown-out buffer keeps part of it.
    let mut token = Zeroizing::new(String::with_capacity(SESSION_TOKEN_PREFIX.len() + RANDOM_HEX));
    token.push_str(SESSION_TOKEN_PREFIX);
    token.push_str(&random);
    let changed = tx.execute(
        "INSERT INTO gw_session_tokens(session_id, owner_id, host_id, hat_id, token_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(session_id) DO UPDATE SET host_id = excluded.host_id, hat_id = excluded.hat_id,
             token_hash = excluded.token_hash, created_at = excluded.created_at, last_used_at = NULL,
             revoked_at = NULL
             WHERE gw_session_tokens.owner_id = excluded.owner_id",
        params![session_id, owner_id, host_id, hat_id, token_hash(&token), now],
    )?;
    // Another owner's row under this id: never taken over.
    anyhow::ensure!(changed == 1, "session {session_id} has a token of another owner's");
    Ok(SessionToken(token))
}

/// Revoke the token of `session_id`, inside the caller's transaction.
/// True if a live one was revoked; revoking twice, or a session that never
/// had one, is not an error.
pub fn revoke_in(tx: &Transaction<'_>, owner_id: &str, session_id: &str, now: i64) -> Result<bool> {
    let changed = tx.execute(
        "UPDATE gw_session_tokens SET revoked_at = ?3
         WHERE session_id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
        params![session_id, owner_id, now],
    )?;
    Ok(changed == 1)
}

/// Revoke every live token of `host_id` (a host revoke, ACP core §4.8),
/// inside the caller's transaction: how many were.
pub fn revoke_host_in(tx: &Transaction<'_>, owner_id: &str, host_id: &str, now: i64) -> Result<usize> {
    Ok(tx.execute(
        "UPDATE gw_session_tokens SET revoked_at = ?3
         WHERE host_id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
        params![host_id, owner_id, now],
    )?)
}

/// Delete every token of `hat_id`, revoked or not, inside the caller's
/// transaction: the gateway's part of a hat purge (gateway spec §2, kernel
/// spec §5.5), before the hat row goes. Idempotent.
pub fn purge_hat_in(tx: &Transaction<'_>, owner_id: &str, hat_id: &str) -> Result<usize> {
    Ok(tx.execute(
        "DELETE FROM gw_session_tokens WHERE hat_id = ?1 AND owner_id = ?2",
        params![hat_id, owner_id],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_is_the_prefix_and_64_lowercase_hex_digits() {
        let good = format!("{SESSION_TOKEN_PREFIX}{}", "a1".repeat(32));
        assert!(is_session_token(&good));
        for bad in [
            String::new(),
            SESSION_TOKEN_PREFIX.to_string(),
            format!("{SESSION_TOKEN_PREFIX}{}", "a1".repeat(31)),
            format!("{SESSION_TOKEN_PREFIX}{}0", "a1".repeat(32)),
            format!("{SESSION_TOKEN_PREFIX}{}", "A1".repeat(32)),
            format!("hnry_client_{}", "a1".repeat(32)),
            format!("{}{}", SESSION_TOKEN_PREFIX.to_uppercase(), "a1".repeat(32)),
        ] {
            assert!(!is_session_token(&bad), "{bad}");
        }
    }

    #[test]
    fn a_token_s_debug_shows_nothing_of_it() {
        let token = SessionToken(Zeroizing::new(format!("{SESSION_TOKEN_PREFIX}{}", "ab".repeat(32))));
        let shown = format!("{token:?} {token:#?}");
        assert!(!shown.contains("abab"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }
}
```

- [ ] **Step 5: Run them to see them pass**

Run: `cargo test -p hennery-gateway --locked --test tokens --test store --lib`
Expected: PASS. Then `cargo test -p hennery-testkit --locked --test owner_filter`: PASS, `tokens.rs` and `scope.rs` audited.

- [ ] **Step 6: Revert-probes**

Each line below, broken as shown, must make the named test fail; restore it after each. (`.superpowers/sdd/8d/probes.py` ran them all.)

- `mint-supersedes-hash` (`tokens.rs`): `mint_in` keeps the old hash on conflict (`token_hash = excluded.token_hash` dropped) → `the_next_mint_supersedes_the_session_s_token` fails.
- `mint-unrevokes` (`tokens.rs`): `mint_in` leaves `revoked_at` on conflict (`revoked_at = NULL` dropped) → `the_next_mint_supersedes_the_session_s_token` fails.
- `mint-owner-guard` (`tokens.rs`): `mint_in`'s upsert drops its owner guard (`WHERE gw_session_tokens.owner_id = excluded.owner_id`) → `a_mint_never_takes_over_another_owner_s_token` fails.
- `mint-clears-used` (`tokens.rs`): `mint_in` keeps `last_used_at` on conflict (`last_used_at = NULL` dropped) → `use_is_recorded_at_most_once_a_minute` fails.
- `revoke` (`tokens.rs`): `revoke_in`'s update matches nothing (`AND 0 = 1`) → `a_revoked_token_resolves_to_nothing_and_revoking_twice_is_fine` fails.
- `revoke-host` (`tokens.rs`): `revoke_host_in`'s update matches nothing → `a_host_revoke_revokes_that_host_s_tokens_only` fails.
- `purge-hat` (`tokens.rs`): `purge_hat_in`'s delete matches nothing → `a_hat_purge_takes_its_tokens_and_then_the_hat_can_go` fails.
- `token-debug` (`tokens.rs`): `SessionToken`'s `Debug` prints the token → `tokens::tests::a_token_s_debug_shows_nothing_of_it` fails.
- `resolve-revoked` (`scope.rs`): the resolve drops `t.revoked_at IS NULL` → `a_revoked_token_resolves_to_nothing_and_revoking_twice_is_fine` fails.
- `resolve-host-revoked` (`scope.rs`): the resolve drops `h.revoked_at IS NULL` → `a_revoked_host_s_tokens_resolve_to_nothing` fails.
- `touch` (`scope.rs`): `last_used_at` never written (`if false && …`) → `use_is_recorded_at_most_once_a_minute` fails.
- `touch-throttle` (`scope.rs`): `last_used_at` written on every use (`now - at >= 0`) → `use_is_recorded_at_most_once_a_minute` fails.
- `scope-hat` (`scope.rs`): the scope query drops the hat (`c.hat_id = ?3 OR 1`) → `out_of_scope_is_one_404` fails.
- `scope-host` (`scope.rs`): the scope query drops the host (`m.host_id = ?4 OR 1`) → `out_of_scope_is_one_404` fails.
- `scope-mounted` (`scope.rs`): the scope query drops the mount (`1 OR EXISTS …`) → `scope_is_the_hat_s_connections_mounted_on_the_host` fails.
- `scope-host-revoked` (`scope.rs`): the scope query drops the mount's `h.revoked_at IS NULL` → `scope_is_the_hat_s_connections_mounted_on_the_host` fails.
- `mark-ok-url` (`scope.rs`): `mark_ok` drops `url = ?3` → `live_status_is_only_for_the_connection_as_it_was` fails.
- `mark-ok-credential` (`scope.rs`): `mark_ok` drops the credential's `EXISTS` → `live_status_is_only_for_the_connection_as_it_was` fails.
- `mark-needs-auth-url` (`scope.rs`): `mark_needs_auth` drops `url = ?3` → `live_status_is_only_for_the_connection_as_it_was` fails.
- `scoped-debug-url` (`scope.rs`): `ScopedConnection`'s `Debug` prints the whole URL → `no_token_credential_or_url_secret_is_logged_or_answered` fails.
- `oc-resolve-ok` (`scope.rs`): the resolve finds nothing (`AND 0 = 1`) → `a_minted_token_resolves_to_its_session_host_and_hat` fails.
- `oc-resolve-unknown` (`scope.rs`): the resolve ignores the token hash → `out_of_scope_is_one_404` fails.
- `oc-scope-ok` (`scope.rs`): the scope query finds nothing (`AND 0 = 1`) → `scope_is_the_hat_s_connections_mounted_on_the_host` fails.

- [ ] **Step 7: The full checks, then commit**

```bash
git add crates/hennery-gateway
git commit -m "feat(gateway): session tokens, ClientIdentity and MountPolicy"
```

### Task 2: The proxy

**Files:**
- Create: `crates/hennery-gateway/src/jsonrpc.rs`, `crates/hennery-gateway/src/proxy.rs`, `crates/hennery-gateway/tests/support/upstream.rs`, `crates/hennery-gateway/tests/proxy.rs`, `crates/hennery-gateway/tests/proxy_log.rs`, `crates/hennery-gateway/tests/proxy_egress.rs`
- Modify: `crates/hennery-gateway/src/lib.rs`, `crates/hennery-gateway/Cargo.toml`, `Cargo.lock`, `crates/hennery-gateway/tests/support/mod.rs`

**Anchors:** `Egress`, `EgressClient::send_streaming`, `EgressClient::send`, `Allowance`, `Limiter`, `Permit`, `Timeouts` (kernel `egress.rs`, 8b); `GatewayStore::static_credential` and `StaticCredential` (8a's R2: the token with its URL, header, prefix and flag in one statement); `model::url_for_logs`; `hennery_proto::rest::ApiError`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-gateway/tests/proxy.rs`:

```rust
//! The proxy (gateway spec §5, §11) against a fake streamable-HTTP MCP
//! upstream, both on loopback, the connection marked `internal_network`
//! (lane L7). Every test drives the proxy over real TCP, so streaming is
//! what a client sees.

mod support;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use hennery_gateway::model::{CredKind, NewConnection};
use hennery_gateway::proxy::Limits;
use serde_json::{Value, json};
use std::time::Duration;
use support::upstream::{FakeUpstream, Harness, call, event, first_then_block, json, list, listed, sse};

/// One hat, one host, one connection to `upstream` mounted there, and a
/// session token for it.
struct Setup {
    h: Harness,
    upstream: FakeUpstream,
    id: String,
    token: String,
}

async fn setup(kind: CredKind, allowlist: Option<&[&str]>) -> Setup {
    setup_with(Harness::new().await, kind, allowlist).await
}

async fn setup_with(h: Harness, kind: CredKind, allowlist: Option<&[&str]>) -> Setup {
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let hat = h.hat();
    let id = h.connection_in("linear", &upstream.url("/mcp"), kind, &hat, allowlist);
    h.mount(&id, &["host-a"]);
    if kind == CredKind::Static {
        h.set_token(&id, "upstream-secret-token");
    }
    let token = h.mint("s1", "host-a", &hat);
    Setup { h, upstream, id, token }
}

fn ping(id: i64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "ping" })
}

/// Gateway spec §5.2: the request goes up with the allowed headers and the
/// static credential, never the client's token, cookie or anything else;
/// the answer comes down with the allowed headers and `nosniff`.
#[tokio::test]
async fn a_request_goes_up_with_the_credential_and_the_allowed_headers_only() {
    let s = setup(CredKind::Static, None).await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", "upstream-session-1")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::SET_COOKIE, "tracker=1")
            .header(header::WWW_AUTHENTICATE, "Bearer realm=x")
            .header("x-upstream-internal", "1")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#))
            .unwrap()
    });
    let resp =
        s.h.client
            .post(format!("{}?leak=1", s.h.url("linear")))
            .bearer_auth(&s.token)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", "upstream-session-1")
            .header("mcp-protocol-version", "2025-06-18")
            .header("last-event-id", "41")
            .header(header::COOKIE, "hennery_session=browser-cookie")
            .header(header::ORIGIN, "https://evil.example")
            .header("x-forwarded-for", "10.0.0.1")
            .header(header::ACCEPT_ENCODING, "gzip, br")
            .header(header::HOST, "evil.example")
            .body(ping(1).to_string())
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let headers = resp.headers().clone();
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(headers["mcp-session-id"], "upstream-session-1");
    // Plan 8d decision 19: never the upstream's caching, always `no-store`.
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    for dropped in [
        "set-cookie",
        "www-authenticate",
        "x-upstream-internal",
        "content-encoding",
    ] {
        assert!(!headers.contains_key(dropped), "{dropped} came down");
    }
    assert_eq!(resp.text().await.unwrap(), r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);

    let seen = s.upstream.seen();
    assert_eq!(seen.len(), 1);
    let up = &seen[0];
    assert_eq!(up.method, "POST");
    // The connection's URL, not the client's query.
    assert_eq!(up.uri, "/mcp");
    assert_eq!(up.header("authorization"), Some("Bearer upstream-secret-token"));
    assert_eq!(up.header("content-type"), Some("application/json"));
    assert_eq!(up.header("accept"), Some("application/json, text/event-stream"));
    assert_eq!(up.header("mcp-session-id"), Some("upstream-session-1"));
    assert_eq!(up.header("mcp-protocol-version"), Some("2025-06-18"));
    assert_eq!(up.header("last-event-id"), Some("41"));
    assert_eq!(up.header("accept-encoding"), Some("identity"));
    for dropped in ["cookie", "origin", "x-forwarded-for"] {
        assert_eq!(up.header(dropped), None, "{dropped} went up");
    }
    // The stored URL's authority, verbatim, never the client's `Host` (plan
    // 8b-ii's obligations to the proxy).
    let authority = s.upstream.url("/mcp");
    let authority = authority.trim_start_matches("http://").trim_end_matches("/mcp");
    assert_eq!(up.header("host"), Some(authority));
    assert_eq!(up.json(), ping(1));
    let everything = format!("{:?} {}", up.headers, String::from_utf8_lossy(&up.body));
    assert!(!everything.contains(&s.token), "the session token went upstream");
}

/// Maintainer decision 6d: a static token under another header, with its
/// prefix; then no `Authorization` goes up at all.
#[tokio::test]
async fn a_static_token_under_its_own_header_and_no_authorization_upstream() {
    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let id = h.connection_with(NewConnection {
        slug: "keyed".into(),
        label: "Keyed".into(),
        url: upstream.url("/mcp"),
        hat_id: h.hat(),
        cred_kind: CredKind::Static,
        static_header: Some("X-API-Key".into()),
        static_prefix: Some("key=".into()),
        tool_allowlist: None,
        internal_network: true,
    });
    h.mount(&id, &["host-a"]);
    h.set_token(&id, "k123");
    let token = h.mint("s1", "host-a", &h.hat());
    let resp = h.post("keyed", &token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let up = &upstream.seen()[0];
    assert_eq!(up.header("x-api-key"), Some("key=k123"));
    assert_eq!(up.header("authorization"), None);
}

#[tokio::test]
async fn a_none_connection_sends_no_credential() {
    let s = setup(CredKind::None, None).await;
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let up = &s.upstream.seen()[0];
    assert_eq!(up.header("authorization"), None);
    assert!(!format!("{:?}", up.headers).contains(&s.token));
    // A client that sends no `Accept` (by hand: reqwest always sends one)
    // gets the streamable-HTTP default.
    let body = ping(2).to_string();
    let mut stream = tokio::net::TcpStream::connect(s.h.addr).await.unwrap();
    let request = format!(
        "POST /mcp/linear HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        s.h.addr,
        s.token,
        body.len()
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .unwrap();
    let mut answer = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut answer)
        .await
        .unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let up = &s.upstream.seen()[1];
    assert_eq!(up.header("accept"), Some("application/json, text/event-stream"));
    // The review's O6: what goes up is typed as what the gateway checked.
    assert_eq!(up.header("content-type"), Some("application/json"));
    s.h.client
        .post(s.h.url("linear"))
        .bearer_auth(&s.token)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-16")
        .body(ping(3).to_string())
        .send()
        .await
        .unwrap();
    let up = &s.upstream.seen()[2];
    let types: Vec<_> = up.headers.get_all("content-type").iter().collect();
    assert_eq!(types, ["application/json"]);
}

/// Gateway spec §3.1, §11's scope negative tests: every way out of scope
/// is the same 404, and the upstream sees nothing.
#[tokio::test]
async fn out_of_scope_is_one_404() {
    let s = setup(CredKind::None, None).await;
    let h = &s.h;
    h.host("host-b", 2);
    let hat = h.hat();
    let work = h.other_hat();
    let theirs = h.connection_in("theirs", &s.upstream.url("/mcp"), CredKind::None, &work, None);
    h.mount(&theirs, &["host-a"]);
    let elsewhere = h.connection("elsewhere", &s.upstream.url("/mcp"), CredKind::None);
    h.mount(&elsewhere, &["host-b"]);
    let revoked = h.mint("s2", "host-a", &hat);
    h.revoke("s2");
    let superseded = h.mint("s3", "host-a", &hat);
    h.mint("s3", "host-a", &hat);
    let on_b = h.mint("s4", "host-b", &hat);
    let unknown = format!("hnry_session_{}", "0".repeat(64));

    let ask = async |slug: &str, auth: Option<String>| {
        let mut req = h
            .client
            .post(h.url(slug))
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth {
            req = req.header(header::AUTHORIZATION, auth);
        }
        let resp = req.body(ping(1).to_string()).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{slug}");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        assert_eq!(resp.headers()["cache-control"], "no-store");
        assert!(!resp.headers().contains_key("www-authenticate"));
        resp.text().await.unwrap()
    };
    let bearer = |t: &str| Some(format!("Bearer {t}"));
    let bodies = vec![
        ask("linear", None).await,
        ask("linear", Some(format!("Basic {}", s.token))).await,
        ask("linear", Some(s.token.clone())).await,
        ask("linear", bearer(&unknown)).await,
        ask("linear", bearer(&revoked)).await,
        ask("linear", bearer(&superseded)).await,
        // Another hat's connection, mounted on this host.
        ask("theirs", bearer(&s.token)).await,
        // This hat's, mounted on another host; and this one from that host.
        ask("elsewhere", bearer(&s.token)).await,
        ask("linear", bearer(&on_b)).await,
        ask("nothing", bearer(&s.token)).await,
        // A slug that is not UTF-8 (the whole-branch review).
        ask("%FF", bearer(&s.token)).await,
    ];
    // Two `Authorization` headers, the first a live token: none is taken.
    let doubled = h
        .client
        .post(h.url("linear"))
        .header(header::AUTHORIZATION, format!("Bearer {}", s.token))
        .header(header::AUTHORIZATION, format!("Bearer {unknown}"))
        .body(ping(1).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(doubled.status(), StatusCode::NOT_FOUND);
    assert_eq!(doubled.text().await.unwrap(), bodies[0]);
    // The review's O7: anything deeper under a slug is the same 404, a
    // bare trailing slash too (the Task 2 review's finding 2).
    for path in ["/mcp/linear/", "/mcp/linear/extra", "/mcp/linear/a/b"] {
        let deeper = h
            .client
            .get(format!("http://{}{path}", h.addr))
            .bearer_auth(&s.token)
            .send()
            .await
            .unwrap();
        assert_eq!(deeper.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(deeper.headers()["x-content-type-options"], "nosniff");
        assert_eq!(deeper.text().await.unwrap(), bodies[0], "{path}");
    }
    // Any other method, with a live token, is the same 404; `HEAD` too,
    // which axum would hand to the `GET` handler (the Task 2 review's
    // finding 1). Nothing goes up, so nothing moves the status.
    for method in ["PUT", "PATCH", "OPTIONS"] {
        let other = h
            .client
            .request(method.parse().unwrap(), h.url("linear"))
            .bearer_auth(&s.token)
            .send()
            .await
            .unwrap();
        assert_eq!(other.status(), StatusCode::NOT_FOUND, "{method}");
        assert_eq!(other.text().await.unwrap(), bodies[0], "{method}");
    }
    let head = h
        .client
        .head(h.url("linear"))
        .bearer_auth(&s.token)
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::NOT_FOUND);
    assert_eq!(h.status(&s.id), "not_connected");
    assert!(bodies.iter().all(|b| b == &bodies[0]), "{bodies:?}");
    let body: Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(body["code"], "not_found");
    // Unmounted now, and a host revoked: refused at once.
    h.mount(&s.id, &[]);
    assert_eq!(ask("linear", bearer(&s.token)).await, bodies[0]);
    h.mount(&s.id, &["host-a"]);
    h.hosts.revoke("host-a", hennery_kernel::secret::unix_now()).unwrap();
    assert_eq!(ask("linear", bearer(&s.token)).await, bodies[0]);
    assert!(s.upstream.seen().is_empty(), "the upstream was reached");
}

/// Plan 8d decision 3: a static connection without its token sends nothing.
#[tokio::test]
async fn a_static_connection_without_a_token_is_502_and_sends_nothing() {
    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let id = h.connection("linear", &upstream.url("/mcp"), CredKind::Static);
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &h.hat());
    let resp = h.post("linear", &token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_auth");
    assert!(upstream.seen().is_empty());
}

/// Gateway spec §5.4: a 401 is never passed on, nor its
/// `WWW-Authenticate`: 502 `upstream_auth` naming the connection, and the
/// connection needs sign-in again. A 2xx later sets it `ok` (§7).
#[tokio::test]
async fn an_upstream_401_is_502_upstream_auth_and_needs_auth() {
    for kind in [CredKind::Static, CredKind::None] {
        let s = setup(kind, None).await;
        s.upstream.reply(|_, _| {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(
                    header::WWW_AUTHENTICATE,
                    r#"Bearer resource_metadata="https://x/.well-known""#,
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap()
        });
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "{kind:?}");
        assert!(!resp.headers().contains_key("www-authenticate"));
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["code"], "upstream_auth");
        assert_eq!(
            body["message"],
            "connection Label linear needs re-authorization in hennery"
        );
        assert_eq!(s.h.status(&s.id), "needs_auth", "{kind:?}");
        // Sent once: there is nothing to refresh.
        assert_eq!(s.upstream.seen().len(), 1);

        s.upstream
            .reply(|_, _| json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}})));
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(s.h.status(&s.id), "ok", "{kind:?}");
    }
}

/// Gateway spec §7: a 2xx through a connection that was not `ok` sets it,
/// and other answers leave it.
#[tokio::test]
async fn live_traffic_sets_ok_on_a_2xx_only() {
    let s = setup(CredKind::None, None).await;
    assert_eq!(s.h.status(&s.id), "not_connected");
    s.upstream
        .reply(|_, _| json(StatusCode::INTERNAL_SERVER_ERROR, &json!({})));
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    // Non-401 errors pass through (§11).
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(s.h.status(&s.id), "not_connected");
    // The review's O5: a body-less 2xx proves nothing.
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap()
    });
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    assert_eq!(s.h.post("linear", &s.token, &note).await.status(), StatusCode::ACCEPTED);
    assert_eq!(s.h.status(&s.id), "not_connected");
    // The same without a `Content-Length` (chunked, ending at once).
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::from_stream(futures::stream::empty::<
                Result<Vec<u8>, std::io::Error>,
            >()))
            .unwrap()
    });
    assert_eq!(s.h.post("linear", &s.token, &note).await.status(), StatusCode::ACCEPTED);
    assert_eq!(s.h.status(&s.id), "not_connected");
    // Nor does an empty body under a JSON type (the Task 2 review).
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, "0")
            .body(Body::empty())
            .unwrap()
    });
    assert_eq!(s.h.post("linear", &s.token, &ping(1)).await.status(), StatusCode::OK);
    assert_eq!(s.h.status(&s.id), "not_connected");
    s.upstream.reply(|_, _| json(StatusCode::OK, &json!({})));
    s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(s.h.status(&s.id), "ok");
}

/// Gateway spec §5.7: a redirect is never followed, nor passed on.
#[tokio::test]
async fn a_redirect_is_502_and_never_followed() {
    let s = setup(CredKind::Static, None).await;
    let target = FakeUpstream::start().await;
    let location = target.url("/stolen");
    s.upstream.reply(move |_, _| {
        Response::builder()
            .status(StatusCode::FOUND)
            .header(header::LOCATION, location.clone())
            .body(Body::empty())
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    assert!(!resp.headers().contains_key("location"));
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_redirect");
    assert!(target.seen().is_empty(), "the redirect was followed");
}

/// Gateway spec §5.2: only JSON and event streams pass, uncompressed; a
/// body-less 202 or 204 has no type to judge (plan 8d decision 4).
#[tokio::test]
async fn only_json_and_event_streams_pass() {
    let s = setup(CredKind::None, None).await;
    let refused = [
        ("text/html", None, StatusCode::OK),
        ("text/plain", None, StatusCode::NOT_FOUND),
        ("application/json", Some("gzip"), StatusCode::OK),
        ("", None, StatusCode::OK),
    ];
    for (content_type, encoding, status) in refused {
        s.upstream.reply(move |_, _| {
            let mut resp = Response::builder().status(status);
            if !content_type.is_empty() {
                resp = resp.header(header::CONTENT_TYPE, content_type);
            }
            if let Some(encoding) = encoding {
                resp = resp.header(header::CONTENT_ENCODING, encoding);
            }
            resp.body(Body::from("<script>alert(1)</script>")).unwrap()
        });
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY, "{content_type} {encoding:?}");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(body["code"], "upstream_content_type");
    }
    // A notification's 202, without a body or a type.
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Body::empty())
            .unwrap()
    });
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let resp = s.h.post("linear", &s.token, &note).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    // A `DELETE` ends the upstream session (G-16): forwarded, 204 back.
    s.upstream.reply(|_, _| {
        Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap()
    });
    let resp =
        s.h.client
            .delete(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header("mcp-session-id", "upstream-session-9")
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let deleted = s.upstream.seen().pop().unwrap();
    assert_eq!(deleted.method, "DELETE");
    assert_eq!(deleted.header("mcp-session-id"), Some("upstream-session-9"));
}

/// Gateway spec §5.3: the first chunk reaches the client before the
/// upstream finishes. The upstream sends one chunk and blocks; a buffering
/// proxy fails this within seconds, it does not hang.
#[tokio::test]
async fn the_first_chunk_arrives_before_the_upstream_finishes() {
    for (content_type, first) in [
        ("application/json", r#"{"jsonrpc":"2.0","#.to_string()),
        (
            "text/event-stream",
            event(&json!({"jsonrpc": "2.0", "method": "notifications/progress"})),
        ),
    ] {
        let s = setup(CredKind::None, None).await;
        let chunk = first.clone();
        s.upstream
            .reply(move |_, hold| first_then_block(content_type, &chunk, hold));
        let resp = tokio::time::timeout(Duration::from_secs(5), s.h.post("linear", &s.token, &ping(1)))
            .await
            .expect("the head was held");
        assert_eq!(resp.status(), StatusCode::OK);
        let mut body = resp.bytes_stream();
        let got = tokio::time::timeout(Duration::from_secs(5), futures::StreamExt::next(&mut body))
            .await
            .unwrap_or_else(|_| panic!("{content_type}: the first chunk was held until the upstream finished"))
            .unwrap()
            .unwrap();
        assert_eq!(got, first.as_bytes(), "{content_type}");
    }
}

/// Gateway spec §5.2: `GET`, the server-to-client channel, is forwarded and
/// streamed; open streams are capped per connection (§5.7), and a closed
/// one frees its place.
#[tokio::test]
async fn get_streams_are_forwarded_and_capped() {
    let s = setup_with(
        Harness::with_limits(Limits::new(8, 1, Duration::from_secs(10), Duration::from_secs(2))).await,
        CredKind::None,
        None,
    )
    .await;
    let note = event(&json!({"jsonrpc": "2.0", "method": "notifications/message"}));
    let chunk = note.clone();
    s.upstream
        .reply(move |_, hold| first_then_block("text/event-stream", &chunk, hold));
    let open = || {
        s.h.client
            .get(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header(header::ACCEPT, "text/event-stream")
            .send()
    };
    let first = open().await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["content-type"], "text/event-stream");
    // No body, so no content type, goes up with a `GET`.
    assert_eq!(s.upstream.seen()[0].header("content-type"), None);
    let mut body = first.bytes_stream();
    let got = futures::StreamExt::next(&mut body).await.unwrap().unwrap();
    assert_eq!(got, note.as_bytes());
    assert_eq!(s.upstream.seen()[0].method, "GET");
    let second = open().await.unwrap();
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let refused: Value = second.json().await.unwrap();
    assert_eq!(refused["code"], "busy");
    assert_eq!(s.upstream.seen().len(), 1, "the refused stream went upstream");
    drop(body);
    // The place frees once the proxy sees the client gone.
    let mut status = StatusCode::SERVICE_UNAVAILABLE;
    for _ in 0..100 {
        status = open().await.unwrap().status();
        if status == StatusCode::OK {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, StatusCode::OK);
}

/// Gateway spec §5.7: requests in flight are capped per connection, held
/// until their body ends.
#[tokio::test]
async fn requests_past_the_cap_are_503() {
    let s = setup_with(
        Harness::with_limits(Limits::new(1, 8, Duration::from_secs(10), Duration::from_secs(2))).await,
        CredKind::None,
        None,
    )
    .await;
    s.upstream
        .reply(|_, hold| first_then_block("application/json", "{", hold));
    let first = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(first.status(), StatusCode::OK);
    let second = s.h.post("linear", &s.token, &ping(2)).await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(s.upstream.seen().len(), 1);
    drop(first);
    let mut status = StatusCode::SERVICE_UNAVAILABLE;
    s.upstream.reply(|_, _| json(StatusCode::OK, &json!({})));
    for _ in 0..100 {
        status = s.h.post("linear", &s.token, &ping(3)).await.status();
        if status == StatusCode::OK {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, StatusCode::OK);
}

/// Gateway spec §5.1: a request body over 4 MiB is 413, whether declared
/// or streamed, and nothing goes up.
#[tokio::test]
async fn a_body_over_4_mib_is_413() {
    let s = setup(CredKind::None, None).await;
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#,
        "x".repeat(4 * 1024 * 1024)
    );
    let resp =
        s.h.client
            .post(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(big.clone())
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        big.as_bytes().chunks(64 * 1024).map(|c| Ok(c.to_vec())).collect();
    let resp =
        s.h.client
            .post(s.h.url("linear"))
            .bearer_auth(&s.token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "body_too_large");
    assert!(s.upstream.seen().is_empty());
    // Just under the cap passes.
    let fits = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#,
        "x".repeat(4 * 1024 * 1024 - 100)
    );
    let resp =
        s.h.client
            .post(s.h.url("linear"))
            .bearer_auth(&s.token)
            .body(fits)
            .send()
            .await
            .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Plan 8d decision 6: a body that is not JSON, or has a key twice, is
/// refused before anything goes up.
#[tokio::test]
async fn a_body_that_is_not_json_or_has_a_key_twice_is_400() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    for body in [
        "not json",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"ping","params":{"name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"ping",}"#,
        // A key the gateway reads, spelt otherwise: a decoder that ignores
        // case (Go's) reads `delete` (the whole-branch review).
        r#"{"jsonrpc":"2.0","id":1,"METHOD":"tools/call","params":{"name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","NAME":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","Params":{"name":"delete"}}"#,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","na_me":"delete"}}"#,
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"capabilities\":{\"\u{17f}ampling\":{}}}}",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"Capabilities":{"sampling":{}}}}"#,
    ] {
        let resp =
            s.h.client
                .post(s.h.url("linear"))
                .bearer_auth(&s.token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{body}");
        let answer: Value = resp.json().await.unwrap();
        assert_eq!(answer["code"], "invalid_request");
    }
    assert!(s.upstream.seen().is_empty());
}

/// Gateway spec §5.5 (G-18): a `tools/call` outside the allowlist is
/// answered by the gateway, -32602, and never reaches the upstream; one
/// inside it does.
#[tokio::test]
async fn a_tools_call_outside_the_allowlist_never_reaches_the_upstream() {
    let s = setup(CredKind::Static, Some(&["search"])).await;
    let resp = s.h.post("linear", &s.token, &call(5, "delete_everything")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let answer: Value = resp.json().await.unwrap();
    assert_eq!(answer["id"], 5);
    assert_eq!(answer["error"]["code"], -32602);
    assert_eq!(answer["error"]["message"], "tool not available through hennery");
    // In a batch too.
    let batch = json!([call(6, "search"), call(7, "delete_everything")]);
    let resp = s.h.post("linear", &s.token, &batch).await;
    let answers: Value = resp.json().await.unwrap();
    assert_eq!(answers[1]["error"]["code"], -32602);
    // A batch with nothing to answer, its refused call a notification:
    // 202, no body (decision 7).
    let note = json!([{"jsonrpc": "2.0", "method": "tools/call", "params": {"name": "delete_everything"}}]);
    let resp = s.h.post("linear", &s.token, &note).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(resp.text().await.unwrap(), "");
    assert!(s.upstream.seen().is_empty(), "a refused call reached the upstream");
    s.upstream.reply(|_, _| {
        json(
            StatusCode::OK,
            &json!({"jsonrpc": "2.0", "id": 8, "result": {"content": []}}),
        )
    });
    let resp = s.h.post("linear", &s.token, &call(8, "search")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(s.upstream.seen().len(), 1);
}

/// Gateway spec §5.5: `tools/list` filtered to the allowlist in JSON, in a
/// JSON batch, and in an event stream (other events byte for byte); a list
/// matching nothing is `[]`.
#[tokio::test]
async fn tools_list_is_filtered_in_json_batches_and_event_streams() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    s.upstream
        .reply(|_, _| json(StatusCode::OK, &listed(1, &["search", "delete", "admin"])));
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    let body: Value = resp.json().await.unwrap();
    let names: Vec<&str> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["search"]);

    s.upstream.reply(|_, _| {
        json(
            StatusCode::OK,
            &json!([listed(1, &["delete"]), {"jsonrpc": "2.0", "id": 2, "result": {}}]),
        )
    });
    let resp = s.h.post("linear", &s.token, &json!([list(1), ping(2)])).await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body[0]["result"]["tools"], json!([]));

    // The review's O1: an answer to id 1 written as 1.0 is filtered too.
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":1.0,"result":{"tools":[{"name":"delete"}]}}"#,
            ))
            .unwrap()
    });
    assert_eq!(body[1], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
    let renumbered: Value = s.h.post("linear", &s.token, &list(1)).await.json().await.unwrap();
    assert_eq!(renumbered["result"]["tools"], json!([]));

    let progress = "id: 1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{\"p\":1}}\n\n";
    let answer = format!("id: 2\n{}", event(&listed(3, &["admin", "search"])));
    let events = [progress.to_string(), answer];
    s.upstream.reply(move |_, _| sse(&events));
    let resp = s.h.post("linear", &s.token, &list(3)).await;
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let text = resp.text().await.unwrap();
    assert!(text.starts_with(progress), "{text}");
    let data = text[progress.len()..]
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap();
    let filtered: Value = serde_json::from_str(data).unwrap();
    assert_eq!(filtered["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["result"]["tools"][0]["name"], "search");
    assert!(text[progress.len()..].starts_with("id: 2\n"), "{text}");
}

/// Without an allowlist nothing is filtered: the bytes as they came.
#[tokio::test]
async fn without_an_allowlist_tools_list_passes_as_it_came() {
    let s = setup(CredKind::None, None).await;
    let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete"}]}}"#;
    s.upstream.reply(move |_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(raw))
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    assert_eq!(resp.text().await.unwrap(), raw);
}

/// Gateway spec §5.3: a `tools/list` answer read whole to filter it is at
/// most 8 MiB, and an error past it, never truncated.
#[tokio::test]
async fn a_filtered_tools_list_over_8_mib_is_502() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    s.upstream.reply(|_, _| {
        let pad = "x".repeat(8 * 1024 * 1024);
        json(
            StatusCode::OK,
            &json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": [{"name": "search", "description": pad}]}}),
        )
    });
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_too_large");
}

/// Gateway spec §5.6 (G-19): `initialize` goes up without `sampling`,
/// `elicitation` and `roots`.
#[tokio::test]
async fn initialize_goes_up_without_the_capabilities_not_forwarded() {
    let s = setup(CredKind::None, None).await;
    let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18",
        "capabilities": {"sampling": {}, "elicitation": {}, "roots": {"listChanged": true}},
        "clientInfo": {"name": "claude-code", "version": "1"}}});
    s.h.post("linear", &s.token, &init).await;
    let up = s.upstream.seen()[0].json();
    assert_eq!(up["params"]["capabilities"], json!({}));
    assert_eq!(up["params"]["clientInfo"]["name"], "claude-code");
}

/// Gateway spec §5.6: a server-to-client request for sampling, in a
/// stream, is answered by the gateway with an error on the same upstream
/// session, and never reaches the client.
#[tokio::test]
async fn a_server_request_for_sampling_is_answered_and_not_passed_on() {
    let s = setup(CredKind::Static, None).await;
    let sampling = event(&json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
    let result = event(&json!({"jsonrpc": "2.0", "id": 1, "result": {"content": []}}));
    let events = [sampling, result.clone()];
    s.upstream.reply(move |seen, _| {
        if seen.body.windows(5).any(|w| w == b"error") {
            Response::builder()
                .status(StatusCode::ACCEPTED)
                .body(Body::empty())
                .unwrap()
        } else {
            let mut resp = sse(&events);
            resp.headers_mut().insert("mcp-session-id", "up-7".parse().unwrap());
            resp
        }
    });
    let resp = s.h.post("linear", &s.token, &call(1, "search")).await;
    assert_eq!(resp.text().await.unwrap(), result);
    let mut answered = None;
    for _ in 0..100 {
        answered = s
            .upstream
            .seen()
            .into_iter()
            .find(|seen| seen.json().get("error").is_some());
        if answered.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let answered = answered.expect("the sampling request was not answered");
    assert_eq!(answered.method, "POST");
    assert_eq!(answered.json()["id"], "srv-1");
    assert_eq!(answered.json()["error"]["code"], -32601);
    assert_eq!(answered.header("mcp-session-id"), Some("up-7"));
    assert_eq!(answered.header("authorization"), Some("Bearer upstream-secret-token"));
}

/// Plan 8d decision 9: the egress allowance is the connection's stored
/// `internal_network` flag, read at every request, and nothing the request
/// carries. Not marked, a loopback upstream is refused before any
/// connection is opened; marked by the operator, the next request goes.
#[tokio::test]
async fn the_egress_allowance_is_the_connection_s_stored_flag() {
    let h = Harness::new().await;
    h.host("host-a", 1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = accepted.clone();
    let accepting = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            drop(socket);
        }
    });
    let id = h.connection_with(NewConnection {
        slug: "lan".into(),
        label: "LAN".into(),
        url: format!("https://127.0.0.1:{port}/mcp"),
        hat_id: h.hat(),
        cred_kind: CredKind::None,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    });
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &h.hat());
    let ask = async || {
        h.client
            .post(h.url("lan"))
            .bearer_auth(&token)
            .header(header::CONTENT_TYPE, "application/json")
            // Nothing a request says moves it to the internal network.
            .header("x-internal-network", "true")
            .header("x-hennery-allowance", "internal_network")
            .body(ping(1).to_string())
            .send()
            .await
            .unwrap()
    };
    let resp = ask().await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a public-only request connected"
    );
    let patch = hennery_gateway::model::ConnectionPatch {
        internal_network: Some(true),
        ..Default::default()
    };
    h.store.update(&id, &patch, hennery_kernel::secret::unix_now()).unwrap();
    // Plain TCP behind an https URL: the handshake fails, but it connected.
    assert_eq!(ask().await.status(), StatusCode::BAD_GATEWAY);
    assert!(
        accepted.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the marked connection never connected"
    );
    accepting.abort();
}

/// Gateway spec §5.3: an event stream is passed on event by event, and one
/// event is at most 8 MiB: past that without its end, the stream ends.
#[tokio::test]
async fn an_event_over_8_mib_ends_the_stream() {
    let s = setup(CredKind::None, None).await;
    s.upstream.reply(|_, hold| {
        let big = format!("data: {}", "x".repeat(8 * 1024 * 1024 + 1));
        first_then_block("text/event-stream", &big, hold)
    });
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let read = tokio::time::timeout(Duration::from_secs(10), resp.bytes())
        .await
        .expect("the stream went on past 8 MiB without an event's end");
    assert!(read.is_err(), "the stream ended cleanly");
}

/// The review's B1: the answer's type is the one the gateway judged it by,
/// exactly; a parameter cannot smuggle another, and two types are refused.
#[tokio::test]
async fn the_answer_s_type_is_the_one_the_gateway_judged() {
    let s = setup(CredKind::None, None).await;
    for (sent, seen) in [
        ("application/json; charset=utf-8", "application/json"),
        ("application/json; x=text/event-stream", "application/json"),
        ("Text/Event-Stream; charset=utf-8", "text/event-stream"),
    ] {
        s.upstream.reply(move |_, _| {
            Response::builder()
                .header(header::CONTENT_TYPE, sent)
                .body(Body::from("{}"))
                .unwrap()
        });
        let resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.status(), StatusCode::OK, "{sent}");
        let types: Vec<_> = resp.headers().get_all("content-type").iter().collect();
        assert_eq!(types, [seen], "{sent}");
    }
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from("{}"))
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_content_type");
}

/// The review's B2: an event stream fails closed. A refused server request
/// behind a byte-order mark, in data serde_json cannot read, or in a last
/// event the stream never ends, never reaches the client.
#[tokio::test]
async fn an_event_stream_fails_closed_on_what_it_cannot_read() {
    let s = setup(CredKind::None, None).await;
    let sampling = r#"{"jsonrpc":"2.0","id":"srv-1","method":"sampling/createMessage","params":{}}"#;
    let surrogate = r#"{"jsonrpc":"2.0","id":"srv-2","method":"sampling/createMessage","params":{"x":"\ud800"}}"#;
    let result = event(&json!({"jsonrpc": "2.0", "id": 1, "result": {}}));
    let streams = [
        format!("\u{feff}data: {sampling}\n\n{result}"),
        format!("data: {surrogate}\n\n{result}"),
        format!("{result}data: {sampling}\n"),
        // The re-confirmation's note 1: a mark that would start the client's
        // stream, behind a stripped mark or a dropped event.
        format!("\u{feff}\u{feff}data: {sampling}\n\n{result}"),
        format!("data: not json\n\n\u{feff}data: {sampling}\n\n{result}"),
        // A key spelt otherwise: serde_json reads no method, a client that
        // ignores case reads sampling (the whole-branch review).
        format!(
            "data: {{\"jsonrpc\":\"2.0\",\"id\":\"srv-4\",\"METHOD\":\"sampling/createMessage\",\"params\":{{}}}}\n\n{result}"
        ),
        // A key twice: serde_json reads `ping`, a client may read sampling
        // (the Task 2 review's finding 3).
        format!(
            "data: {{\"jsonrpc\":\"2.0\",\"id\":\"srv-3\",\"method\":\"sampling/createMessage\",\"method\":\"ping\",\"params\":{{}}}}\n\n{result}"
        ),
    ];
    for body in streams {
        let sent = body.clone();
        s.upstream.reply(move |_, _| {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(sent.clone()))
                .unwrap()
        });
        let text = s.h.post("linear", &s.token, &ping(1)).await.text().await.unwrap();
        assert_eq!(text, result, "{body:?}");
    }
    // The one behind the mark was read, and answered.
    let mut answered = false;
    for _ in 0..100 {
        answered = s
            .upstream
            .seen()
            .iter()
            .any(|seen| seen.body.windows(5).any(|w| w == b"srv-1"));
        if answered {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(answered, "the request behind the mark was not answered");
}

/// The review's O3: a request body has `body_timeout` to arrive, and a
/// stalled one is 408; nothing goes up.
#[tokio::test]
async fn a_body_that_does_not_arrive_in_time_is_408() {
    let s = setup(CredKind::None, None).await;
    let mut stream = tokio::net::TcpStream::connect(s.h.addr).await.unwrap();
    let request = format!(
        "POST /mcp/linear HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{\"jsonrpc\"",
        s.h.addr, s.token
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .unwrap();
    let mut answer = vec![0u8; 4096];
    let read = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::io::AsyncReadExt::read(&mut stream, &mut answer),
    )
    .await
    .expect("no answer to a stalled body")
    .unwrap();
    let answer = String::from_utf8_lossy(&answer[..read]);
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    assert!(answer.contains("request_timeout"), "{answer}");
    assert!(s.upstream.seen().is_empty());
}

/// The review's O3: a `GET` stream takes a stream permit, not a request
/// one, so open channels never starve a connection's requests.
#[tokio::test]
async fn a_get_stream_takes_no_request_permit() {
    let s = setup_with(
        Harness::with_limits(Limits::new(1, 1, Duration::from_secs(10), Duration::from_secs(2))).await,
        CredKind::None,
        None,
    )
    .await;
    s.upstream.reply(|seen, hold| {
        if seen.method == "GET" {
            first_then_block("text/event-stream", ": open\n\n", hold)
        } else {
            json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}))
        }
    });
    let stream =
        s.h.client
            .get(s.h.url("linear"))
            .bearer_auth(&s.token)
            .send()
            .await
            .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(s.h.post("linear", &s.token, &ping(1)).await.status(), StatusCode::OK);
    drop(stream);
}

/// Plan 8d decision 10: the response head has `head_timeout` to arrive (300
/// s by default, past the egress client's own deadline); an upstream that
/// takes the request and never answers is 502 `upstream_unreachable` then
/// (the Task 2 review's finding 4).
#[tokio::test]
async fn an_upstream_that_never_answers_is_502_after_the_head_timeout() {
    let h = Harness::with_limits(Limits::new(8, 8, Duration::from_millis(300), Duration::from_secs(2))).await;
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = silent.local_addr().unwrap();
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        loop {
            let (conn, _) = silent.accept().await.unwrap();
            open.push(conn);
        }
    });
    h.host("host-a", 1);
    let hat = h.hat();
    let id = h.connection_in("silent", &format!("http://{addr}/mcp"), CredKind::None, &hat, None);
    h.mount(&id, &["host-a"]);
    let token = h.mint("s1", "host-a", &hat);
    let resp = tokio::time::timeout(Duration::from_secs(5), h.post("silent", &token, &ping(1)))
        .await
        .expect("no answer within 5 s: the head timeout did not fire");
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");
    held.abort();
}

/// Plan 8d decision 14: a refused server request's answer goes on the
/// request's own `Mcp-Session-Id` when the upstream's answer names none;
/// and at the connection's request cap it is skipped, not queued (the Task
/// 2 review's finding 5).
#[tokio::test]
async fn a_refused_server_request_s_answer_takes_the_request_s_session_and_a_permit() {
    for cap in [2, 1] {
        let s = setup_with(
            Harness::with_limits(Limits::new(cap, 8, Duration::from_secs(10), Duration::from_secs(2))).await,
            CredKind::None,
            None,
        )
        .await;
        let sampling =
            event(&json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
        s.upstream.reply(move |seen, hold| {
            if seen.body.windows(5).any(|w| w == b"error") {
                Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(Body::empty())
                    .unwrap()
            } else {
                // The sampling request, then nothing: the open stream holds
                // its request permit.
                first_then_block("text/event-stream", &sampling, hold)
            }
        });
        let resp =
            s.h.client
                .post(s.h.url("linear"))
                .bearer_auth(&s.token)
                .header(header::CONTENT_TYPE, "application/json")
                .header("mcp-session-id", "client-session-9")
                .body(ping(1).to_string())
                .send()
                .await
                .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let answered = || {
            s.upstream
                .seen()
                .into_iter()
                .find(|seen| seen.json().get("error").is_some())
        };
        let mut found = None;
        for _ in 0..50 {
            found = answered();
            if found.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if cap == 2 {
            let found = found.expect("the sampling request was not answered");
            assert_eq!(found.header("mcp-session-id"), Some("client-session-9"));
        } else {
            assert!(found.is_none(), "answered past the request cap");
        }
        drop(resp);
    }
}

/// Plan 8b-ii's obligation, nothing cached across a `PATCH` (the
/// whole-branch review): a refused server request's answer reads the
/// connection again, so a request that arrives on a stream opened before
/// the connection changed is left unanswered: its URL moved, its internal
/// marking went (written to the row: the API keeps an `http` URL marked),
/// it was unmounted, or another connection has its slug and URL now.
#[tokio::test]
async fn a_refused_server_request_after_its_connection_changed_is_left_unanswered() {
    for change in ["moved", "unmarked", "unmounted", "replaced"] {
        let s = setup(CredKind::None, None).await;
        let moved_to = FakeUpstream::start().await;
        let (go, wait) = tokio::sync::watch::channel(false);
        let sampling =
            event(&json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}}));
        let replies = move |seen: &support::upstream::Seen, _: &tokio::sync::watch::Receiver<()>| {
            if seen.body.windows(5).any(|w| w == b"error") {
                return Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(Body::empty())
                    .unwrap();
            }
            // An open stream; the server request only once the test says so.
            let (wait, sampling) = (wait.clone(), sampling.clone());
            let body = futures::stream::unfold(0, move |step| {
                let (mut wait, sampling) = (wait.clone(), sampling.clone());
                async move {
                    match step {
                        0 => Some((Ok::<_, std::io::Error>(": open\n\n".to_string()), 1)),
                        1 => {
                            wait.wait_for(|go| *go).await.unwrap();
                            Some((Ok(sampling), 2))
                        }
                        _ => None,
                    }
                }
            });
            Response::builder()
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(body))
                .unwrap()
        };
        s.upstream.reply(replies);
        let mut resp = s.h.post("linear", &s.token, &ping(1)).await;
        assert_eq!(resp.chunk().await.unwrap().unwrap(), ": open\n\n", "{change}");
        let now = hennery_kernel::secret::unix_now();
        match change {
            "moved" => {
                let patch = hennery_gateway::model::ConnectionPatch {
                    url: Some(moved_to.url("/mcp")),
                    ..Default::default()
                };
                s.h.store.update(&s.id, &patch, now).unwrap();
            }
            "unmarked" => {
                let changed =
                    s.h.raw()
                        .execute("UPDATE gw_connections SET internal_network = 0 WHERE id = ?1", [&s.id])
                        .unwrap();
                assert_eq!(changed, 1);
            }
            "unmounted" => s.h.mount(&s.id, &[]),
            _ => {
                assert!(s.h.store.delete(&s.id).unwrap());
                let again = s.h.connection("linear", &s.upstream.url("/mcp"), CredKind::None);
                s.h.mount(&again, &["host-a"]);
            }
        }
        go.send(true).unwrap();
        // The request is not passed on, and the stream ends.
        while resp.chunk().await.unwrap().is_some() {}
        tokio::time::sleep(Duration::from_millis(500)).await;
        let answered = |up: &FakeUpstream| up.seen().iter().any(|seen| seen.body.windows(5).any(|w| w == b"error"));
        assert!(!answered(&s.upstream), "{change}: answered at the stream's URL");
        assert!(!answered(&moved_to), "{change}: answered at the new URL");
    }
}

/// Gateway spec §5.1: a body declared over 4 MiB is 413 at once, before
/// any of it is read; a client that never sends it is not kept waiting for
/// the body timeout (408).
#[tokio::test]
async fn a_body_declared_over_4_mib_is_413_before_it_is_read() {
    let s = setup(CredKind::None, None).await;
    let mut stream = tokio::net::TcpStream::connect(s.h.addr).await.unwrap();
    let request = format!(
        "POST /mcp/linear HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        s.h.addr,
        s.token,
        4 * 1024 * 1024 + 1
    );
    tokio::io::AsyncWriteExt::write_all(&mut stream, request.as_bytes())
        .await
        .unwrap();
    let mut answer = vec![0u8; 4096];
    let read = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::io::AsyncReadExt::read(&mut stream, &mut answer),
    )
    .await
    .expect("no answer within 1 s: the body was waited for")
    .unwrap();
    let answer = String::from_utf8_lossy(&answer[..read]);
    assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
    assert!(answer.contains("body_too_large"), "{answer}");
    assert!(s.upstream.seen().is_empty());
}

/// Plan 8d: a `tools/list` answer in JSON that must be filtered, and does
/// not parse, is 502 `upstream_invalid`; nothing of it is passed on.
#[tokio::test]
async fn a_filtered_tools_list_that_does_not_parse_is_502() {
    let s = setup(CredKind::None, Some(&["search"])).await;
    s.upstream.reply(|_, _| {
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"delete"}"#,
            ))
            .unwrap()
    });
    let resp = s.h.post("linear", &s.token, &list(1)).await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_invalid");
}

/// Plan 8d decision 13: what the gateway cannot read of its own store is
/// 500 `internal`, an `ApiError`, and nothing goes up.
#[tokio::test]
async fn a_connection_the_store_holds_damaged_is_500_internal() {
    let s = setup(CredKind::None, None).await;
    let changed =
        s.h.raw()
            .execute("UPDATE gw_connections SET url = 'not a url' WHERE id = ?1", [&s.id])
            .unwrap();
    assert_eq!(changed, 1);
    let resp = s.h.post("linear", &s.token, &ping(1)).await;
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "internal");
    assert!(s.upstream.seen().is_empty());
}
```

Create `crates/hennery-gateway/tests/proxy_egress.rs`:

```rust
//! Plan 8b-ii's obligation to the proxy: a connection not marked
//! `internal_network` is refused plain `http` to a LAN address, by the
//! egress policy, before anything is sent. The API never stores that (plan
//! 8a saves `http` only on a marked connection), so the row is written as a
//! damaged or older store might hold it. The refusal and a failed connect
//! answer the same 502, so the test reads the proxy's log line: a binary
//! of its own, the subscriber being the process's.

mod support;

use axum::http::StatusCode;
use hennery_gateway::model::CredKind;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::upstream::Harness;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unmarked_connection_is_refused_plain_http_to_a_lan_address() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let h = Harness::new().await;
    h.host("host-a", 1);
    // An RFC 1918 address: nothing here answers it.
    let id = h.connection("lan", "http://10.255.255.1:9/mcp", CredKind::None);
    h.mount(&id, &["host-a"]);
    let changed = h
        .raw()
        .execute("UPDATE gw_connections SET internal_network = 0 WHERE id = ?1", [&id])
        .unwrap();
    assert_eq!(changed, 1);
    let token = h.mint("s1", "host-a", &h.hat());

    let resp = h
        .post("lan", &token, &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "upstream_unreachable");

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|line| line.contains("gateway proxy: the upstream was not reached"))
        .unwrap_or_else(|| panic!("no warning:\n{logs}"));
    assert!(line.contains("http://10.255.255.1:9"), "{line}");
    assert!(
        line.contains("refused by the egress policy"),
        "sent, not refused: {line}"
    );
}
```

Create `crates/hennery-gateway/tests/proxy_log.rs`:

```rust
//! Token and URL hygiene through the proxy (gateway spec §3.1, §11; lane
//! L8, L11): the session token, the static credential and the secret parts
//! of an upstream URL (its path and query) appear in no log line at any
//! level, no answer to the client and no `Debug` the proxy prints, whatever
//! the request comes to. A binary of its own: the subscriber is the
//! process's, so the server's tasks on every worker thread log into it.

mod support;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use hennery_gateway::model::CredKind;
use hennery_gateway::scope::MountPolicy;
use serde_json::json;
use std::sync::{Arc, Mutex};
use support::upstream::{FakeUpstream, Harness, event, first_then_fail, json, sse};

const PATH_SECRET: &str = "pathcanary0123456789";
const QUERY_SECRET: &str = "querycanary9876543210";
const CREDENTIAL: &str = "credcanary-5f3e2d1c0b";

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_token_credential_or_url_secret_is_logged_or_answered() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let h = Harness::new().await;
    let upstream = FakeUpstream::start().await;
    h.host("host-a", 1);
    let secret_url = upstream.url(&format!("/mcp/{PATH_SECRET}?key={QUERY_SECRET}"));
    let id = h.connection("linear", &secret_url, CredKind::Static);
    h.mount(&id, &["host-a"]);
    h.set_token(&id, CREDENTIAL);
    // A connection to a port nothing listens on, its URL secret too.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let gone = h.connection(
        "gone",
        &format!("http://127.0.0.1:{closed}/mcp/{PATH_SECRET}?key={QUERY_SECRET}"),
        CredKind::Static,
    );
    h.mount(&gone, &["host-a"]);
    h.set_token(&gone, CREDENTIAL);
    let token = h.mint("s1", "host-a", &h.hat());

    let mut answers = String::new();
    let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
    let mut exchange = async |slug: &str, token: &str| {
        let resp = h.post(slug, token, &ping).await;
        answers.push_str(&format!("{} {:?} ", resp.status(), resp.headers()));
        answers.push_str(&resp.text().await.unwrap_or_default());
    };
    let replies: Vec<Box<dyn Fn() -> Response + Send + Sync>> = vec![
        Box::new(|| json(StatusCode::OK, &json!({"jsonrpc": "2.0", "id": 1, "result": {}}))),
        Box::new(|| {
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Bearer")
                .body(Body::empty())
                .unwrap()
        }),
        Box::new(|| {
            Response::builder()
                .status(StatusCode::FOUND)
                .header(header::LOCATION, "http://127.0.0.1:1/x")
                .body(Body::empty())
                .unwrap()
        }),
        Box::new(|| {
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html")
                .body(Body::from("<p>"))
                .unwrap()
        }),
        Box::new(|| first_then_fail("application/json", "{")),
        Box::new(|| first_then_fail("text/event-stream", "data: {}\n\n")),
        Box::new(|| sse(&[event(&json!({"jsonrpc": "2.0", "id": "x", "method": "roots/list"}))])),
    ];
    for reply in replies {
        let reply = Arc::new(reply);
        upstream.reply(move |_, _| reply());
        exchange("linear", &token).await;
    }
    exchange("gone", &token).await;
    exchange("linear", "hnry_session_0000").await;
    exchange("nothing", &token).await;

    // A scoped connection's `Debug`, as a log line would print it.
    let scoped = h
        .proxy_store
        .connection(
            &hennery_gateway::scope::Principal {
                hat_id: h.hat(),
                kind: hennery_gateway::scope::PrincipalKind::Session {
                    session_id: "s1".into(),
                    host_id: "host-a".into(),
                },
            },
            "linear",
        )
        .unwrap()
        .unwrap();
    let debug = format!("{scoped:?} {scoped:#?}");
    // The upstream answering the refused `roots/list` in the background.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    // The capture works: the proxy's lines are in it, with the connection
    // and the upstream's origin.
    assert!(logs.contains(&id), "{logs}");
    assert!(
        logs.contains("gateway proxy: the upstream refused the credential"),
        "{logs}"
    );
    assert!(logs.contains("gateway proxy: the upstream was not reached"), "{logs}");
    // The cut bodies' errors were logged, so their lines were checked too.
    assert!(logs.contains("gateway proxy: the upstream body failed"), "{logs}");
    assert!(logs.contains("gateway proxy: the upstream stream failed"), "{logs}");
    assert!(logs.contains(&format!("http://127.0.0.1:{closed}")), "{logs}");
    assert!(answers.contains("upstream_unreachable"), "{answers}");
    assert!(debug.contains("linear"), "{debug}");
    for secret in [PATH_SECRET, QUERY_SECRET, CREDENTIAL, token.as_str()] {
        assert!(!logs.contains(secret), "{secret} logged:\n{logs}");
        assert!(!answers.contains(secret), "{secret} answered:\n{answers}");
        assert!(!debug.contains(secret), "{secret} in a Debug:\n{debug}");
    }
}
```

In `crates/hennery-gateway/tests/support/mod.rs`, replace:

```rust
#![allow(dead_code)]
```

with:

```rust
#![allow(dead_code)]

pub mod upstream;
```

Create `crates/hennery-gateway/tests/support/upstream.rs`:

```rust
//! A fake streamable-HTTP MCP upstream, and the proxy served over loopback
//! on a `World` (plan 8d). Connections reach the fake under
//! `internal_network`, as every test connection must (lane L7): there is
//! no test-only bypass.

use super::World;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::Response;
use hennery_gateway::proxy::{Limits, ProxyState, router};
use hennery_kernel::egress::{Egress, Timeouts};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// One request the fake upstream received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// Path and query, as sent.
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|v| v.to_str().unwrap())
    }
}

type Handler = Arc<dyn Fn(&Seen, &watch::Receiver<()>) -> Response + Send + Sync>;

/// The fake upstream: records every request and answers with whatever the
/// test's handler builds. A body that blocks waits on the hold, which is
/// released when the fake is dropped.
pub struct FakeUpstream {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    handler: Arc<Mutex<Handler>>,
    release: Option<watch::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeUpstream {
    pub async fn start() -> Self {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let default: Handler = Arc::new(|_, _| json(StatusCode::OK, &serde_json::json!({})));
        let handler = Arc::new(Mutex::new(default));
        let (release, hold) = watch::channel(());
        let app = {
            let seen = seen.clone();
            let handler = handler.clone();
            Router::new().fallback(move |req: Request<Body>| {
                let seen = seen.clone();
                let handler = handler.clone();
                let hold = hold.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap().to_vec();
                    let one = Seen {
                        method: parts.method.to_string(),
                        uri: parts.uri.to_string(),
                        headers: parts.headers,
                        body,
                    };
                    seen.lock().unwrap().push(one.clone());
                    let handler = handler.lock().unwrap().clone();
                    handler(&one, &hold)
                }
            })
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            seen,
            handler,
            release: Some(release),
            task,
        }
    }

    /// Answer every request with `handler`'s response.
    pub fn reply(&self, handler: impl Fn(&Seen, &watch::Receiver<()>) -> Response + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Arc::new(handler);
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.release.take();
        self.task.abort();
    }
}

/// A JSON answer.
pub fn json(status: StatusCode, value: &serde_json::Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap()
}

/// An event stream of `events`, each a complete event's text.
pub fn sse(events: &[String]) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(Body::from(events.concat()))
        .unwrap()
}

/// One `data:` event holding `value`.
pub fn event(value: &serde_json::Value) -> String {
    format!("event: message\ndata: {value}\n\n")
}

/// A body of `content_type` that sends `first` and then blocks until the
/// fake is dropped (gateway spec §5.3's streaming test).
pub fn first_then_block(content_type: &str, first: &str, hold: &watch::Receiver<()>) -> Response {
    let first = Bytes::from(first.to_string());
    let hold = hold.clone();
    let stream = futures::stream::unfold(Some(first), move |state| {
        let mut hold = hold.clone();
        async move {
            match state {
                Some(first) => Some((Ok::<_, std::io::Error>(first), None)),
                None => {
                    // Until the sender is dropped.
                    while hold.changed().await.is_ok() {}
                    None
                }
            }
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// A body that sends `first` and then, once the head and that chunk have
/// gone out, fails, cutting the connection.
pub fn first_then_fail(content_type: &str, first: &str) -> Response {
    let first = Bytes::from(first.to_string());
    let stream = futures::stream::unfold(Some(first), |state| async move {
        match state {
            Some(first) => Some((Ok(first), None)),
            None => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Some((Err(std::io::Error::other("cut")), None))
            }
        }
    });
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(stream))
        .unwrap()
}

/// The proxy over loopback, on a `World` (its fields and helpers through
/// `Deref`).
pub struct Harness {
    pub world: World,
    pub addr: SocketAddr,
    pub client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}

impl std::ops::Deref for Harness {
    type Target = World;

    fn deref(&self) -> &World {
        &self.world
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_limits(Limits::new(8, 8, Duration::from_secs(10), Duration::from_secs(2))).await
    }

    pub async fn with_limits(limits: Limits) -> Self {
        let world = World::new();
        let egress = Egress::new(Timeouts {
            connect: Duration::from_secs(2),
            request: Duration::from_secs(10),
        })
        .unwrap();
        let app = router(ProxyState::full(
            world.proxy_store.clone(),
            world.store.clone(),
            world.key.clone(),
            egress,
            limits,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        Self {
            world,
            addr,
            client,
            task,
        }
    }

    pub fn url(&self, slug: &str) -> String {
        format!("http://{}/mcp/{slug}", self.addr)
    }

    /// `POST /mcp/<slug>` with `token`, a JSON-RPC `body`.
    pub async fn post(&self, slug: &str, token: &str, body: &serde_json::Value) -> reqwest::Response {
        self.client
            .post(self.url(slug))
            .bearer_auth(token)
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::ACCEPT,
                HeaderValue::from_static("application/json, text/event-stream"),
            )
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }
}

/// A `tools/call` of `name`.
pub fn call(id: i64, name: &str) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": {} } })
}

/// A `tools/list` request.
pub fn list(id: i64) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list" })
}

/// A `tools/list` answer naming `tools`.
pub fn listed(id: i64, tools: &[&str]) -> serde_json::Value {
    let tools: Vec<serde_json::Value> = tools
        .iter()
        .map(|name| serde_json::json!({ "name": name, "inputSchema": { "type": "object" } }))
        .collect();
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": { "tools": tools } })
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-gateway --locked --test proxy --test proxy_log --test proxy_egress`
Expected: FAIL to compile: unresolved import `hennery_gateway::proxy`.

- [ ] **Step 3: Commit the tests**

```bash
git add crates/hennery-gateway/tests
git commit -m "test(gateway): the proxy against a fake streamable-HTTP MCP upstream"
```

- [ ] **Step 4: The proxy**

In `Cargo.lock`, replace:

```toml
 "hennery-kernel",
 "hennery-proto",
 "hex",
 "libc",
 "rusqlite",
```

with:

```toml
 "futures",
 "hennery-kernel",
 "hennery-proto",
 "hex",
 "libc",
 "reqwest",
 "rusqlite",
 "serde",
```

In `crates/hennery-gateway/Cargo.toml`, replace:

```toml
hennery-kernel.workspace = true
hennery-proto.workspace = true
hex.workspace = true
libc = "0.2"
rusqlite.workspace = true
serde_json.workspace = true
thiserror.workspace = true
```

with:

```toml
# The proxy (plan 8d): its bodies are streams.
futures.workspace = true
hennery-kernel.workspace = true
hennery-proto.workspace = true
hex.workspace = true
libc = "0.2"
# The proxy's upstream requests (plan 8d), sent through the kernel's egress
# clients (`hennery_kernel::egress`), never a client of its own.
reqwest.workspace = true
rusqlite.workspace = true
# What the proxy reads of JSON-RPC (plan 8d): duplicate keys refused.
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
# The proxy answers refused server requests in the background (plan 8d).
tokio.workspace = true
```

Create `crates/hennery-gateway/src/jsonrpc.rs`:

```rust
//! What the proxy reads of MCP's JSON-RPC (gateway spec §5.5, §5.6): the
//! tool allowlist on `tools/call` and `tools/list`, and the capabilities
//! not forwarded in v1. Everything else passes as it came.
//!
//! - A request body must be JSON without a duplicate key anywhere (plan 8d
//!   decision 6): a parser upstream that keeps the first of two `"method"`s
//!   or `"name"`s would otherwise run what the allowlist check never saw.
//!   Nor may it spell a key the gateway reads otherwise than exactly
//!   (`METHOD`, `Name`, `ſampling`): a decoder that matches names whatever
//!   their case, as Go's `encoding/json` does, would read it.
//! - `initialize` goes upstream without `sampling`, `elicitation` and
//!   `roots` in its client capabilities.
//! - With an allowlist, a `tools/call` for a tool outside it is answered
//!   here, `-32602`, and never reaches the upstream; the ids of `tools/list`
//!   requests are kept, and their responses filtered.
//! - A server-to-client request for one of those capabilities, arriving in
//!   an event stream, is answered here with an error and not passed on.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Value, json};
use std::collections::HashSet;

/// The client capabilities never sent upstream (gateway spec §5.6).
pub const STRIPPED_CAPABILITIES: &[&str] = &["sampling", "elicitation", "roots"];

/// Server-to-client requests answered with an error, never passed on: the
/// ones those capabilities would have allowed.
pub const REFUSED_SERVER_REQUESTS: &[&str] = &["sampling/createMessage", "elicitation/create", "roots/list"];

/// `tools/call` outside the allowlist (gateway spec §5.5).
pub const TOOL_NOT_AVAILABLE: i64 = -32602;
const TOOL_NOT_AVAILABLE_MESSAGE: &str = "tool not available through hennery";

/// The rest of a batch that held such a call (plan 8d decision 7).
const INVALID_REQUEST: i64 = -32600;
const BATCH_REFUSED_MESSAGE: &str = "batch refused: a tools/call in it is not available through hennery";

/// A server-to-client request the gateway does not forward.
const METHOD_NOT_FOUND: i64 = -32601;

/// A UTF-8 byte-order mark, which a client's event-stream parser strips
/// once, at the start of its stream.
pub const BOM: &[u8] = b"\xef\xbb\xbf";

/// What to do with a request body.
#[derive(Debug, PartialEq)]
pub enum Inspected {
    /// Send it upstream: `body` is the client's bytes, or `initialize`
    /// rewritten. `tools_list` holds the ids of its `tools/list` requests
    /// when the connection has an allowlist, whose responses are filtered.
    Forward { body: Vec<u8>, tools_list: Vec<Value> },
    /// Answer it here and send nothing upstream: a JSON-RPC answer, or
    /// `None` when nothing in it has an id (202, no body).
    Answer(Option<Value>),
    /// Not JSON, a key twice in one object, or a key the gateway reads
    /// spelt otherwise: refused, 400.
    Invalid(&'static str),
}

/// What a `POST` body refused 400 is told.
const INVALID_BODY: &str =
    "the body is not JSON, has a key twice in one object, or spells a key the gateway reads otherwise";

/// The keys the gateway reads in a message, and in the `params` of the
/// methods it reads them for. A decoder that matches names whatever their
/// case (Go's `encoding/json`) takes `METHOD` or `Name` for them, so such
/// a spelling, at its level, is refused like a key twice (plan 8d decision
/// 6; the whole-branch review).
const MESSAGE_KEYS: &[&str] = &["jsonrpc", "id", "method", "params"];

/// A key as such a decoder compares it with the names the gateway reads:
/// ASCII case, and `ſ` as `s`. Unicode folds only one other letter to an
/// ASCII one, the Kelvin sign to `k`, and no name read here has a `k`.
/// Go's `encoding/json/v2`, matching names case-insensitively, ignores `_`
/// and `-` as well (the re-confirmation's F1).
fn folded(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .map(|c| match c {
            '\u{17f}' => 's',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// Whether `object` has a key that folds to one of `names` without being it.
fn respelt(object: &serde_json::Map<String, Value>, names: &[&str]) -> bool {
    object.keys().any(|key| {
        let folded = folded(key);
        names.iter().any(|name| folded == *name && key != name)
    })
}

/// Whether a message, or any in a batch, spells a key the gateway reads
/// otherwise than exactly: one of the message's own, `name` in a
/// `tools/call`'s `params`, or `capabilities` and those not forwarded in
/// an `initialize`'s.
pub fn respelt_keys(value: &Value) -> bool {
    let messages: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    messages.into_iter().any(|message| {
        let Some(object) = message.as_object() else {
            return false;
        };
        if respelt(object, MESSAGE_KEYS) {
            return true;
        }
        let Some(params) = object.get("params").and_then(Value::as_object) else {
            return false;
        };
        match method(message) {
            Some("tools/call") => respelt(params, &["name"]),
            Some("initialize") => {
                respelt(params, &["capabilities"])
                    || params
                        .get("capabilities")
                        .and_then(Value::as_object)
                        .is_some_and(|capabilities| respelt(capabilities, STRIPPED_CAPABILITIES))
            }
            _ => false,
        }
    })
}

/// Inspect a `POST` body under `allowlist` (`None`: every tool).
pub fn inspect_request(body: &[u8], allowlist: Option<&[String]>) -> Inspected {
    if serde_json::from_slice::<Unique>(body).is_err() {
        return Inspected::Invalid(INVALID_BODY);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return Inspected::Invalid(INVALID_BODY);
    };
    if respelt_keys(&value) {
        return Inspected::Invalid(INVALID_BODY);
    }
    let mut rewritten = false;
    let mut blocked = false;
    let mut tools_list = Vec::new();
    {
        let messages: Vec<&mut Value> = match &mut value {
            Value::Array(items) => items.iter_mut().collect(),
            other => vec![other],
        };
        for message in messages {
            match method(message) {
                Some("initialize") => rewritten |= strip_capabilities(message),
                Some("tools/call") if allowlist.is_some_and(|tools| !call_allowed(message, tools)) => blocked = true,
                Some("tools/list") if allowlist.is_some() => {
                    if let Some(id) = message.get("id") {
                        tools_list.push(id.clone());
                    }
                }
                _ => {}
            }
        }
    }
    if blocked {
        return Inspected::Answer(refusal(&value, allowlist.unwrap_or_default()));
    }
    let body = if rewritten {
        serde_json::to_vec(&value).expect("a JSON value serialises")
    } else {
        body.to_vec()
    };
    Inspected::Forward { body, tools_list }
}

fn method(message: &Value) -> Option<&str> {
    message.get("method")?.as_str()
}

/// Whether a `tools/call` names, as a string, a tool in `tools`.
fn call_allowed(message: &Value, tools: &[String]) -> bool {
    message
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|name| tools.iter().any(|tool| tool == name))
}

/// Remove the capabilities not forwarded from an `initialize`: true if one
/// was there.
fn strip_capabilities(message: &mut Value) -> bool {
    let Some(capabilities) = message
        .get_mut("params")
        .and_then(|params| params.get_mut("capabilities"))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut removed = false;
    for name in STRIPPED_CAPABILITIES {
        removed |= capabilities.remove(*name).is_some();
    }
    removed
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The answer to a body holding a `tools/call` outside the allowlist: for
/// one message, its error; for a batch, an error for each request in it,
/// that call's and the others' (plan 8d decision 7). Notifications get
/// none; with nothing to answer, `None`.
fn refusal(value: &Value, tools: &[String]) -> Option<Value> {
    let answer = |message: &Value| {
        let id = message.get("id")?;
        Some(
            if method(message) == Some("tools/call") && !call_allowed(message, tools) {
                error(id, TOOL_NOT_AVAILABLE, TOOL_NOT_AVAILABLE_MESSAGE)
            } else {
                error(id, INVALID_REQUEST, BATCH_REFUSED_MESSAGE)
            },
        )
    };
    match value {
        Value::Array(items) => {
            let answers: Vec<Value> = items.iter().filter_map(answer).collect();
            (!answers.is_empty()).then_some(Value::Array(answers))
        }
        message => answer(message),
    }
}

/// Whether a response's id answers a request's: equal, or the same number
/// however written (`1` and `1.0`), as a client's matching may read them
/// (the review's O1).
fn same_id(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Filter the `tools/list` responses in `value` (a message or a batch)
/// whose id is in `ids` to `tools`: true if one was found. A result without
/// a tools array gets an empty one, never `null`.
pub fn filter_tools_lists(value: &mut Value, ids: &[Value], tools: &[String]) -> bool {
    let messages: Vec<&mut Value> = match value {
        Value::Array(items) => items.iter_mut().collect(),
        other => vec![other],
    };
    let mut found = false;
    for message in messages {
        if !message
            .get("id")
            .is_some_and(|id| ids.iter().any(|want| same_id(id, want)))
        {
            continue;
        }
        let Some(result) = message.get_mut("result").and_then(Value::as_object_mut) else {
            continue;
        };
        found = true;
        let listed = match result.remove("tools") {
            Some(Value::Array(listed)) => listed,
            _ => Vec::new(),
        };
        let kept: Vec<Value> = listed
            .into_iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| tools.iter().any(|t| t == name))
            })
            .collect();
        result.insert("tools".into(), Value::Array(kept));
    }
    found
}

/// The server-to-client requests in `value` (a message or a batch) the
/// gateway refuses, removed from it, each with the error to send back.
/// What is left of `value` is `None` if nothing is.
pub fn take_refused_requests(value: Value) -> (Option<Value>, Vec<Value>) {
    let refused = |message: &Value| {
        let method = method(message)?;
        let id = message.get("id")?;
        REFUSED_SERVER_REQUESTS.contains(&method).then(|| {
            error(
                id,
                METHOD_NOT_FOUND,
                &format!("{method} is not available through hennery"),
            )
        })
    };
    match value {
        Value::Array(items) => {
            let mut answers = Vec::new();
            let mut kept = Vec::new();
            for item in items {
                match refused(&item) {
                    Some(answer) => answers.push(answer),
                    None => kept.push(item),
                }
            }
            if answers.is_empty() {
                (Some(Value::Array(kept)), answers)
            } else {
                ((!kept.is_empty()).then_some(Value::Array(kept)), answers)
            }
        }
        message => match refused(&message) {
            Some(answer) => (None, vec![answer]),
            None => (Some(message), Vec::new()),
        },
    }
}

/// Any JSON, refusing a key twice in one object at any depth.
struct Unique;

impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JSON without duplicate keys")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_i64<E>(self, _: i64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_u64<E>(self, _: u64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_f64<E>(self, _: f64) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_str<E>(self, _: &str) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Unique, A::Error> {
        while seq.next_element::<Unique>()?.is_some() {}
        Ok(Unique)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Unique, A::Error> {
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key) {
                return Err(de::Error::custom("a key twice in one object"));
            }
            map.next_value::<Unique>()?;
        }
        Ok(Unique)
    }
}

/// Where the first complete event in `buf` ends (gateway spec §5.5's SSE
/// framing): `Ok`, just past the empty line that ends it. A line ends in
/// `\r\n`, `\n` or `\r`; a `\r` as the last byte may be half of a `\r\n`
/// still to come, so it ends nothing yet. The scan starts at `from`, the
/// start of a line; without an end, `Err` is where the next scan of the same
/// event, with more bytes, starts (the review's O2: no rescan of a long
/// event from its beginning).
pub fn event_end(buf: &[u8], from: usize) -> Result<usize, usize> {
    let mut line_start = from;
    let mut i = from;
    while i < buf.len() {
        let end = match buf[i] {
            b'\n' => i + 1,
            b'\r' if i + 1 == buf.len() => return Err(line_start),
            b'\r' if buf[i + 1] == b'\n' => i + 2,
            b'\r' => i + 1,
            _ => {
                i += 1;
                continue;
            }
        };
        if i == line_start {
            return Ok(end);
        }
        line_start = end;
        i = end;
    }
    Err(line_start)
}

/// One complete event, as its fields: the data lines' values joined by
/// `\n` (`None` without one), and every other line as it was.
fn event_parts(event: &[u8]) -> (Option<String>, Vec<&[u8]>) {
    let mut data: Option<String> = None;
    let mut others = Vec::new();
    let text_lines = event
        .split(|b| *b == b'\n')
        .flat_map(|line| line.split(|b| *b == b'\r'))
        .filter(|line| !line.is_empty());
    for line in text_lines {
        let value = if line == b"data" {
            Some(&b""[..])
        } else {
            line.strip_prefix(b"data:")
                .map(|rest| rest.strip_prefix(b" ").unwrap_or(rest))
        };
        match value {
            Some(value) => {
                let value = String::from_utf8_lossy(value);
                match &mut data {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(&value);
                    }
                    None => data = Some(value.into_owned()),
                }
            }
            None => others.push(line),
        }
    }
    (data, others)
}

/// What became of one event.
#[derive(Debug, PartialEq)]
pub enum EventOutcome {
    /// Passed on as it came.
    Unchanged,
    /// Passed on as these bytes instead.
    Rewritten(Vec<u8>),
    /// Not passed on.
    Dropped,
    /// Not passed on: its data is not JSON, has a key twice in an object
    /// or spells a key the gateway reads otherwise, or a line of it starts
    /// with a byte-order mark, so the gateway cannot read what a client
    /// might (the review's B2, the re-confirmation's note 1, the Task 2
    /// review's finding 3, the whole-branch review).
    Unreadable,
}

/// Apply the gateway's rules to one complete event: refused server
/// requests taken out (their errors pushed to `answers`), `tools/list`
/// responses whose id is in `ids` filtered to `allowlist`. An event without
/// data (a comment, a ping) or one no rule touches passes as it came, byte
/// for byte; one whose data is not JSON, or has a key twice, is
/// `Unreadable`.
pub fn rewrite_event(
    event: &[u8],
    ids: &[Value],
    allowlist: Option<&[String]>,
    answers: &mut Vec<Value>,
) -> EventOutcome {
    // A line that starts with a byte-order mark: if this event is the first
    // the client sees, its parser strips the mark and reads a field the
    // gateway read as another (the re-confirmation's note 1).
    if event
        .split(|b| *b == b'\n' || *b == b'\r')
        .any(|line| line.starts_with(BOM))
    {
        return EventOutcome::Unreadable;
    }
    let (Some(data), others) = event_parts(event) else {
        return EventOutcome::Unchanged;
    };
    // Not JSON to serde_json, or a key twice in an object: serde_json keeps
    // the last, a client may keep the first (decision 6's reason, for what
    // comes down).
    let read = serde_json::from_str::<Unique>(&data).and_then(|_| serde_json::from_str::<Value>(&data));
    let value = match read {
        Ok(value) => value,
        Err(_) => return EventOutcome::Unreadable,
    };
    // A refused request's `method` spelt otherwise (the whole-branch
    // review): a client whose decoder ignores case would run it.
    if respelt_keys(&value) {
        return EventOutcome::Unreadable;
    }
    let (kept, refused) = take_refused_requests(value);
    let changed = !refused.is_empty();
    answers.extend(refused);
    let Some(mut kept) = kept else {
        return EventOutcome::Dropped;
    };
    let filtered = match allowlist {
        Some(tools) if !ids.is_empty() => filter_tools_lists(&mut kept, ids, tools),
        _ => false,
    };
    if !changed && !filtered {
        return EventOutcome::Unchanged;
    }
    let mut out = Vec::new();
    for line in others {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(&serde_json::to_vec(&kept).expect("a JSON value serialises"));
    out.extend_from_slice(b"\n\n");
    EventOutcome::Rewritten(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<String> {
        vec!["search".into()]
    }

    #[test]
    fn a_body_with_a_key_twice_or_not_json_is_invalid() {
        for body in [
            &br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"ping"}"#[..],
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","name":"delete"}}"#,
            br#"[{"a":1},{"b":{"c":1,"c":2}}]"#,
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","method":"x","method":"y"}"#,
            br#"{"jsonrpc":"2.0","id":1,"method":"ping",}"#,
            b"",
            b"not json",
        ] {
            assert!(
                matches!(inspect_request(body, None), Inspected::Invalid(_)),
                "{}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_tools_call_outside_the_allowlist_is_answered_here() {
        let body = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"delete"}}"#;
        let Inspected::Answer(Some(answer)) = inspect_request(body, Some(&tools())) else {
            panic!("forwarded");
        };
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["error"]["code"], TOOL_NOT_AVAILABLE);
        // Without an allowlist, or for a listed tool, it is forwarded as it came.
        assert_eq!(
            inspect_request(body, None),
            Inspected::Forward {
                body: body.to_vec(),
                tools_list: vec![]
            }
        );
        let ok = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"search"}}"#;
        assert!(matches!(inspect_request(ok, Some(&tools())), Inspected::Forward { .. }));
        // A name that is not a string is not on any list.
        let odd = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":["search"]}}"#;
        assert!(matches!(
            inspect_request(odd, Some(&tools())),
            Inspected::Answer(Some(_))
        ));
        // A notification of one: nothing to answer, nothing sent.
        let note = br#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"delete"}}"#;
        assert_eq!(inspect_request(note, Some(&tools())), Inspected::Answer(None));
    }

    #[test]
    fn a_batch_with_a_refused_call_is_answered_element_by_element() {
        let body = br#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"},
            {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"delete"}},
            {"jsonrpc":"2.0","method":"notifications/x"}]"#;
        let Inspected::Answer(Some(Value::Array(answers))) = inspect_request(body, Some(&tools())) else {
            panic!("forwarded");
        };
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0]["id"], 1);
        assert_eq!(answers[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(answers[1]["id"], 2);
        assert_eq!(answers[1]["error"]["code"], TOOL_NOT_AVAILABLE);
    }

    #[test]
    fn initialize_loses_the_capabilities_not_forwarded() {
        let body = br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18",
            "capabilities":{"sampling":{},"elicitation":{},"roots":{"listChanged":true},"experimental":{"x":1}},
            "clientInfo":{"name":"c","version":"1"}}}"#;
        let Inspected::Forward { body, .. } = inspect_request(body, None) else {
            panic!("not forwarded");
        };
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["params"]["capabilities"], json!({ "experimental": { "x": 1 } }));
        assert_eq!(sent["params"]["clientInfo"]["name"], "c");
        // Nothing to strip: the bytes as they came.
        let plain = br#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"capabilities":{}}}"#;
        assert_eq!(
            inspect_request(plain, None),
            Inspected::Forward {
                body: plain.to_vec(),
                tools_list: vec![]
            }
        );
    }

    #[test]
    fn tools_list_ids_are_kept_only_under_an_allowlist() {
        let body = br#"[{"jsonrpc":"2.0","id":"a","method":"tools/list"},{"jsonrpc":"2.0","id":3,"method":"ping"}]"#;
        let Inspected::Forward { tools_list, .. } = inspect_request(body, Some(&tools())) else {
            panic!("not forwarded");
        };
        assert_eq!(tools_list, vec![json!("a")]);
        let Inspected::Forward { tools_list, .. } = inspect_request(body, None) else {
            panic!("not forwarded");
        };
        assert!(tools_list.is_empty());
    }

    #[test]
    fn a_tools_list_response_is_filtered_and_an_empty_one_is_an_array() {
        let mut value = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": [
            { "name": "search" }, { "name": "delete" }, { "nameless": true }
        ], "nextCursor": "c" } });
        assert!(filter_tools_lists(&mut value, &[json!(1)], &tools()));
        assert_eq!(value["result"]["tools"], json!([{ "name": "search" }]));
        assert_eq!(value["result"]["nextCursor"], "c");
        let mut none = json!({ "jsonrpc": "2.0", "id": 1, "result": { "tools": null } });
        assert!(filter_tools_lists(&mut none, &[json!(1)], &[]));
        assert_eq!(none["result"]["tools"], json!([]));
        // Another id, or an error: untouched.
        let mut other = json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": [{ "name": "delete" }] } });
        assert!(!filter_tools_lists(&mut other, &[json!(1)], &tools()));
        assert_eq!(other["result"]["tools"][0]["name"], "delete");
        // A batch, element by element.
        let mut batch = json!([
            { "jsonrpc": "2.0", "id": 1, "result": { "tools": [{ "name": "delete" }] } },
            { "jsonrpc": "2.0", "id": 2, "result": { "tools": [{ "name": "delete" }] } }
        ]);
        assert!(filter_tools_lists(&mut batch, &[json!(1)], &tools()));
        assert_eq!(batch[0]["result"]["tools"], json!([]));
        assert_eq!(batch[1]["result"]["tools"][0]["name"], "delete");
    }

    #[test]
    fn events_end_at_an_empty_line_of_any_line_ending() {
        assert_eq!(event_end(b"data: a\n\nrest", 0), Ok(9));
        assert_eq!(event_end(b"data: a\r\n\r\nrest", 0), Ok(11));
        assert_eq!(event_end(b"data: a\r\rrest", 0), Ok(9));
        assert_eq!(event_end(b"data: a\n", 0), Err(8));
        assert_eq!(event_end(b"data: a\r\n\r", 0), Err(9));
        assert_eq!(event_end(b"data: a\n\r", 0), Err(8));
        assert_eq!(event_end(b": ping\n\n", 0), Ok(8));
        assert_eq!(event_end(b"\n", 0), Ok(1));
        assert_eq!(event_end(b"data: a", 0), Err(0));
        // Going on from where the last scan stopped finds the same end.
        let event = b"id: 1\ndata: a\r\n\r\nrest";
        for cut in 0..event.len() {
            let mut from = 0;
            if let Err(resume) = event_end(&event[..cut], 0) {
                from = resume;
            }
            assert_eq!(event_end(event, from), Ok(17), "cut at {cut}");
        }
    }

    #[test]
    fn an_event_is_rewritten_only_when_a_rule_applies() {
        let mut answers = Vec::new();
        let plain = b"id: 4\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n\n";
        assert_eq!(
            rewrite_event(plain, &[json!(1)], Some(&tools()), &mut answers),
            EventOutcome::Unchanged
        );
        assert_eq!(
            rewrite_event(b": comment\n\n", &[], None, &mut answers),
            EventOutcome::Unchanged
        );
        assert_eq!(
            rewrite_event(b"data: not json\n\n", &[], None, &mut answers),
            EventOutcome::Unreadable
        );
        // A lone surrogate: JSON to some parsers, not to serde_json.
        assert_eq!(
            rewrite_event(
                b"data: {\"id\":1,\"method\":\"x\",\"p\":\"\\ud800\"}\n\n",
                &[],
                None,
                &mut answers
            ),
            EventOutcome::Unreadable
        );
        let list =
            b"id: 5\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\ndata: \"result\":{\"tools\":[{\"name\":\"delete\"}]}}\n\n";
        let EventOutcome::Rewritten(out) = rewrite_event(list, &[json!(1)], Some(&tools()), &mut answers) else {
            panic!("not rewritten");
        };
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("id: 5\ndata: "), "{out}");
        assert!(out.ends_with("\n\n"), "{out}");
        let data: Value = serde_json::from_str(out.trim_end().strip_prefix("id: 5\ndata: ").unwrap()).unwrap();
        assert_eq!(data["result"]["tools"], json!([]));
        assert!(answers.is_empty());
        let sampling =
            b"data: {\"jsonrpc\":\"2.0\",\"id\":\"s1\",\"method\":\"sampling/createMessage\",\"params\":{}}\n\n";
        assert_eq!(rewrite_event(sampling, &[], None, &mut answers), EventOutcome::Dropped);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0]["id"], "s1");
        assert_eq!(answers[0]["error"]["code"], METHOD_NOT_FOUND);
        // A line that starts with a byte-order mark: unreadable, wherever it is.
        let marked = b"\xef\xbb\xbfdata: {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(marked, &[], None, &mut answers), EventOutcome::Unreadable);
        let later = b"id: 1\n\xef\xbb\xbf: x\ndata: {}\n\n";
        assert_eq!(rewrite_event(later, &[], None, &mut answers), EventOutcome::Unreadable);
        // A notification of that name has no id: nothing to answer, passed on.
        let note = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"roots/list\"}\n\n";
        assert_eq!(rewrite_event(note, &[], None, &mut answers), EventOutcome::Unchanged);
    }
}
```

In `crates/hennery-gateway/src/lib.rs`, replace:

```rust
pub mod key;
pub mod model;
```

with:

```rust
mod jsonrpc;
pub mod key;
pub mod model;
pub mod proxy;
```

Create `crates/hennery-gateway/src/proxy.rs`:

```rust
//! The proxy (gateway spec §5): `POST|GET|DELETE /mcp/<slug>`, with the
//! client's token in `Authorization: Bearer`. Hand-written on axum and
//! reqwest, streaming, through the kernel's egress policy (kernel spec
//! §7.1). It changes nothing but the credential, `initialize`'s
//! capabilities and, under an allowlist, `tools/list` and `tools/call`.
//!
//! - **Outside the operator's routes** (lane L8, kernel spec §3.3: "Exempt"):
//!   no session cookie, no `Origin` rule, no compression layer.
//! - **404 for everything out of scope** (§3.1): no token, an unknown,
//!   revoked or superseded one, a connection that is not mounted on the
//!   session's host or is another hat's, or no such slug. The same body
//!   every time, and never 401: an agent answers a 401 by starting an OAuth
//!   flow of its own (G-17).
//! - **Headers** both ways are allowlists (§5.2). The client's
//!   `Authorization` is the gateway's own token: it never goes upstream.
//!   `Content-Type` is the gateway's both ways: a `POST` goes up as
//!   `application/json`, which it checked, and an answer comes down as the
//!   one type the gateway judged it by (the review's B1).
//! - **Streaming** (§5.3): every body is passed on chunk by chunk, except a
//!   `tools/list` answer in JSON under an allowlist, which is read whole
//!   (8 MiB at most) and filtered. An event stream is passed on event by
//!   event: a complete event is never held, a partial one waits for its end
//!   (plan 8d decision 5).
//! - **401** (§5.4) is never passed on: `502 upstream_auth`. The one place
//!   an OAuth refresh and retry goes is `refreshed` (plan 8f).
//! - **Limits** (§5.7): per connection, the requests in flight and the open
//!   `GET` streams; past either, 503. A request body has 30 s to arrive.
//! - **Errors** the proxy answers itself are `ApiError` (`{code, message}`),
//!   as every other route's: 404 `not_found`; 400 `invalid_request`; 408
//!   `request_timeout`; 413 `body_too_large`; 503 `busy`; 502
//!   `upstream_auth`, `upstream_unreachable`, `upstream_redirect`,
//!   `upstream_content_type`, `upstream_too_large`, `upstream_invalid`; 500
//!   `internal`. A refused `tools/call` is a JSON-RPC error, in a 200.
//! - **Logs** name the connection and its slug, never the token, the
//!   credential or more of the upstream URL than its origin (lane L11).

use crate::jsonrpc::{self, BOM, EventOutcome, Inspected};
use crate::key::MasterKey;
use crate::model::{CredKind, url_for_logs};
use crate::scope::{ClientIdentity, MountPolicy, Principal, ProxyStore, ScopedConnection};
use crate::store::GatewayStore;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use futures::{Stream, StreamExt};
use hennery_kernel::egress::{Allowance, Egress, EgressClient, Limiter, Permit};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::ApiError;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// The largest request body (gateway spec §5.1).
pub const MAX_REQUEST_BODY: usize = 4 * 1024 * 1024;

/// The largest `tools/list` answer read whole to filter it, and the
/// largest single event of a stream (gateway spec §5.3).
pub const MAX_FILTERED_BODY: usize = 8 * 1024 * 1024;

/// What the proxy sends as `Accept` when the client sends none (§5.2).
const DEFAULT_ACCEPT: &str = "application/json, text/event-stream";

/// The request headers passed upstream as they came (gateway spec §5.2).
/// `Content-Type` is the gateway's own (the review's O6).
const FORWARDED_REQUEST_HEADERS: &[&str] = &["accept", "mcp-session-id", "mcp-protocol-version", "last-event-id"];

/// The response headers passed downstream as they came (gateway spec
/// §5.2). `Content-Type` is set from what the gateway judged the body to be
/// (the review's B1), and `Cache-Control` is always `no-store` (plan 8d
/// decision 19).
const FORWARDED_RESPONSE_HEADERS: &[&str] = &["mcp-session-id"];

/// The proxy's limits (gateway spec §5.7; plan 8d decision 10).
#[derive(Clone, Debug)]
pub struct Limits {
    /// Per connection: `POST` and `DELETE` requests in flight, from before
    /// their body is read until their answer's body ends.
    pub requests: Limiter,
    /// Per connection: open `GET` streams, the server-to-client channel,
    /// for their whole life. A `GET` takes no request permit.
    pub streams: Limiter,
    /// How long an upstream may take to send its response head.
    pub head_timeout: Duration,
    /// How long a client may take to send its request body (the review's
    /// O3): a slow body holds a request permit.
    pub body_timeout: Duration,
}

impl Limits {
    pub const MAX_REQUESTS: usize = 64;
    pub const MAX_STREAMS: usize = 32;
    pub const HEAD_TIMEOUT: Duration = Duration::from_secs(300);
    pub const BODY_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new(max_requests: usize, max_streams: usize, head_timeout: Duration, body_timeout: Duration) -> Self {
        Self {
            requests: Limiter::new(max_requests),
            streams: Limiter::new(max_streams),
            head_timeout,
            body_timeout,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(
            Self::MAX_REQUESTS,
            Self::MAX_STREAMS,
            Self::HEAD_TIMEOUT,
            Self::BODY_TIMEOUT,
        )
    }
}

/// What the proxy runs on.
#[derive(Clone)]
pub struct ProxyState {
    /// Token → principal.
    pub identity: Arc<dyn ClientIdentity>,
    /// Principal → connections.
    pub mounts: Arc<dyn MountPolicy>,
    /// Static credentials, with where they go (`static_credential`).
    pub credentials: Arc<GatewayStore>,
    /// What live traffic says of a connection's status.
    pub statuses: Arc<ProxyStore>,
    pub key: Arc<MasterKey>,
    pub egress: Egress,
    pub limits: Limits,
}

impl ProxyState {
    /// Full mode (umbrella §10.2): session tokens and host mounts, both from
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
        }
    }
}

/// The proxy's route. Merge it beside the operator's routes, never under
/// `operator_only` (lane L8). Every answer, the proxy's own errors too,
/// carries `X-Content-Type-Options: nosniff` and `Cache-Control: no-store`.
/// Any other method (`HEAD` too, which axum would hand to the `GET`
/// handler), and anything deeper under a slug, a bare trailing slash
/// included, is the same 404, never another router's (the review's O7; the
/// Task 2 review's findings 1 and 2).
pub fn router(state: ProxyState) -> Router {
    Router::new()
        .route(
            "/mcp/{slug}",
            post(proxy)
                .get(proxy)
                .delete(proxy)
                .head(async || not_found())
                .fallback(async || not_found()),
        )
        .route("/mcp/{slug}/", any(async || not_found()))
        .route("/mcp/{slug}/{*rest}", any(async || not_found()))
        .layer(middleware::map_response(nosniff))
        .layer(middleware::map_response(no_store))
        .with_state(state)
}

async fn nosniff(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

/// An answer to a request that carries a token is no shared cache's to
/// keep, whatever the upstream says (plan 8d decision 19).
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The proxy's own answer: an `ApiError`, as every route's.
fn refuse(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        axum::Json(ApiError {
            code: code.into(),
            message: message.into(),
            session_id: None,
        }),
    )
        .into_response()
}

/// The one answer for everything out of scope.
fn not_found() -> Response {
    refuse(StatusCode::NOT_FOUND, "not_found", "no such MCP server")
}

fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "gateway proxy: internal error");
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

fn upstream_auth(connection: &ScopedConnection) -> Response {
    refuse(
        StatusCode::BAD_GATEWAY,
        "upstream_auth",
        format!("connection {} needs re-authorization in hennery", connection.label),
    )
}

/// The bearer token of `Authorization`, if there is exactly one.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

/// How the request authenticates upstream. Its `Debug` shows no secret.
enum UpstreamAuth {
    /// `cred_kind = none`: no credential header.
    None,
    /// `cred_kind = static`: `<header>: <prefix><token>`.
    Static { header: HeaderName, value: HeaderValue },
}

impl std::fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Static { header, .. } => write!(f, "Static({header}: <redacted>)"),
        }
    }
}

/// Where one request goes and how: read with the credential itself, so an
/// edit cannot move the URL between them (plan 8a's R2).
struct Upstream {
    url: Url,
    /// The connection's stored flag, as read for this request.
    internal_network: bool,
    auth: UpstreamAuth,
}

/// The egress client a connection's requests go through: chosen at every
/// request from the connection's stored `internal_network` flag, never from
/// anything in the request (plan 8d decision 9). The one place the choice
/// is made, so a later client for the internal network swaps in here.
fn egress_client(egress: &Egress, internal_network: bool) -> EgressClient {
    egress.client(if internal_network {
        Allowance::InternalNetwork
    } else {
        Allowance::PublicOnly
    })
}

/// The connection's upstream and credential now. `None`: it has none it
/// can use (a `static` connection without a token, or a kind the proxy
/// does not take yet), so nothing is sent (plan 8d decision 3).
fn upstream(state: &ProxyState, connection: &ScopedConnection) -> anyhow::Result<Option<Upstream>> {
    match connection.cred_kind {
        CredKind::None => Ok(Some(Upstream {
            url: Url::parse(&connection.url)?,
            internal_network: connection.internal_network,
            auth: UpstreamAuth::None,
        })),
        CredKind::Static => {
            let Some(credential) = state.credentials.static_credential(&connection.id, &state.key)? else {
                return Ok(None);
            };
            let header = HeaderName::from_bytes(credential.static_header.as_bytes())?;
            let mut value =
                HeaderValue::from_str(&format!("{}{}", credential.static_prefix, credential.token.as_str()))?;
            value.set_sensitive(true);
            Ok(Some(Upstream {
                url: Url::parse(&credential.url)?,
                internal_network: credential.internal_network,
                auth: UpstreamAuth::Static { header, value },
            }))
        }
        // Plan 8f.
        CredKind::OauthDcr | CredKind::OauthClient => Ok(None),
    }
}

/// The seam for plan 8f: after an upstream 401, a fresh credential to try
/// once more, single-flight per connection. Static and `none` credentials
/// have nothing to refresh.
async fn refreshed(_state: &ProxyState, _connection: &ScopedConnection, auth: &UpstreamAuth) -> Option<UpstreamAuth> {
    match auth {
        UpstreamAuth::None | UpstreamAuth::Static { .. } => None,
    }
}

/// The upstream request's headers: the allowlist, `Accept` defaulted, no
/// compression, `Content-Type: application/json` with a body (the gateway
/// checked it is JSON), and the credential, never the client's
/// `Authorization`.
fn upstream_headers(downstream: &HeaderMap, auth: &UpstreamAuth, json_body: bool) -> HeaderMap {
    let mut out = HeaderMap::new();
    if json_body {
        out.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    for name in FORWARDED_REQUEST_HEADERS {
        for value in downstream.get_all(*name) {
            out.append(HeaderName::from_static(name), value.clone());
        }
    }
    if !out.contains_key(header::ACCEPT) {
        out.insert(header::ACCEPT, HeaderValue::from_static(DEFAULT_ACCEPT));
    }
    out.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    if let UpstreamAuth::Static { header, value } = auth {
        out.insert(header.clone(), value.clone());
    }
    out
}

/// Read `body` whole, refusing past `cap` bytes.
async fn read_capped<S, E>(mut stream: S, cap: usize) -> Result<Vec<u8>, ReadError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
{
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ReadError::Failed)?;
        if out.len() + chunk.len() > cap {
            return Err(ReadError::TooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

enum ReadError {
    TooLarge,
    Failed,
}

/// `POST|GET|DELETE /mcp/{slug}`.
async fn proxy(
    State(state): State<ProxyState>,
    slug: Result<Path<String>, PathRejection>,
    method: Method,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let now = unix_now();
    // A slug that is not UTF-8 is no slug: the same 404 (the whole-branch
    // review), not axum's plain-text 400.
    let Ok(Path(slug)) = slug else {
        return not_found();
    };
    let Some(token) = bearer(&headers) else {
        return not_found();
    };
    let principal = match state.identity.resolve(token, now) {
        Ok(Some(principal)) => principal,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    let connection = match state.mounts.connection(&principal, &slug) {
        Ok(Some(connection)) => connection,
        Ok(None) => return not_found(),
        Err(err) => return internal(err),
    };
    let permit = if method == Method::GET {
        state.limits.streams.try_acquire(&connection.id)
    } else {
        state.limits.requests.try_acquire(&connection.id)
    };
    let Ok(permit) = permit else {
        tracing::warn!(connection_id = %connection.id, slug = %connection.slug, %method, "gateway proxy: too many requests or streams");
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "too many requests or streams to this MCP server",
        );
    };
    let (body, tools_list) = if method == Method::POST {
        let declared = headers
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if declared.is_some_and(|len| len > MAX_REQUEST_BODY as u64) {
            return body_too_large();
        }
        let read = read_capped(body.into_data_stream(), MAX_REQUEST_BODY);
        let bytes = match tokio::time::timeout(state.limits.body_timeout, read).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(ReadError::TooLarge)) => return body_too_large(),
            Ok(Err(ReadError::Failed)) => {
                return refuse(StatusCode::BAD_REQUEST, "invalid_request", "the body was not read");
            }
            Err(_) => {
                return refuse(
                    StatusCode::REQUEST_TIMEOUT,
                    "request_timeout",
                    "the request body did not arrive in time",
                );
            }
        };
        match jsonrpc::inspect_request(&bytes, connection.tool_allowlist.as_deref()) {
            Inspected::Forward { body, tools_list } => (Some(body), tools_list),
            Inspected::Answer(Some(answer)) => {
                tracing::info!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: a tools/call outside the allowlist refused");
                return axum::Json(answer).into_response();
            }
            Inspected::Answer(None) => return StatusCode::ACCEPTED.into_response(),
            Inspected::Invalid(why) => return refuse(StatusCode::BAD_REQUEST, "invalid_request", why),
        }
    } else {
        (None, Vec::new())
    };
    let upstream = match upstream(&state, &connection) {
        Ok(Some(upstream)) => upstream,
        Ok(None) => {
            tracing::info!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: the connection has no credential to send");
            return upstream_auth(&connection);
        }
        Err(err) => return internal(err),
    };
    let client = egress_client(&state.egress, upstream.internal_network);
    let send = |auth: &UpstreamAuth| {
        let mut request = reqwest::Request::new(method.clone(), upstream.url.clone());
        *request.headers_mut() = upstream_headers(&headers, auth, body.is_some());
        *request.body_mut() = body.clone().map(reqwest::Body::from);
        *request.timeout_mut() = Some(state.limits.head_timeout);
        client.send_streaming(request)
    };
    let mut response = send(&upstream.auth).await;
    if matches!(&response, Ok(r) if r.status() == StatusCode::UNAUTHORIZED)
        && let Some(fresh) = refreshed(&state, &connection, &upstream.auth).await
    {
        response = send(&fresh).await;
    }
    let response = match response {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(
                connection_id = %connection.id,
                slug = %connection.slug,
                upstream = %url_for_logs(upstream.url.as_str()),
                error = %err,
                "gateway proxy: the upstream was not reached"
            );
            return refuse(
                StatusCode::BAD_GATEWAY,
                "upstream_unreachable",
                format!("connection {} could not be reached", connection.label),
            );
        }
    };
    let status = response.status();
    tracing::debug!(connection_id = %connection.id, slug = %connection.slug, %method, status = status.as_u16(), "gateway proxy: upstream answered");
    if status == StatusCode::UNAUTHORIZED {
        drop(response);
        tracing::warn!(connection_id = %connection.id, slug = %connection.slug, "gateway proxy: the upstream refused the credential");
        if let Err(err) = state
            .statuses
            .mark_needs_auth(&connection.id, upstream.url.as_str(), now)
        {
            tracing::error!(connection_id = %connection.id, error = %err, "gateway proxy: status not recorded");
        }
        return upstream_auth(&connection);
    }
    if status.is_redirection() {
        tracing::warn!(connection_id = %connection.id, slug = %connection.slug, status = status.as_u16(), "gateway proxy: the upstream redirected, not followed");
        return refuse(
            StatusCode::BAD_GATEWAY,
            "upstream_redirect",
            format!("connection {} answered with a redirect", connection.label),
        );
    }
    let kind = match body_kind(&response) {
        Ok(kind) => kind,
        Err(why) => {
            tracing::warn!(connection_id = %connection.id, slug = %connection.slug, status = status.as_u16(), why, "gateway proxy: the upstream's answer is not passed on");
            return refuse(
                StatusCode::BAD_GATEWAY,
                "upstream_content_type",
                format!("connection {} answered with {why}", connection.label),
            );
        }
    };
    // A body-less 2xx proves nothing: some upstreams take a notification
    // before they check its credential (the review's O5). An empty body
    // under a type is no more (the Task 2 review).
    if status.is_success()
        && !matches!(kind, BodyKind::Empty)
        && response.content_length() != Some(0)
        && let Err(err) = state.statuses.mark_ok(&connection.id, upstream.url.as_str(), now)
    {
        tracing::error!(connection_id = %connection.id, error = %err, "gateway proxy: status not recorded");
    }
    let mut out = HeaderMap::new();
    for name in FORWARDED_RESPONSE_HEADERS {
        for value in response.headers().get_all(*name) {
            out.append(HeaderName::from_static(name), value.clone());
        }
    }
    match kind {
        BodyKind::Empty => {}
        BodyKind::Json => {
            out.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }
        BodyKind::EventStream => {
            out.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
        }
    }
    let session_id = response
        .headers()
        .get("mcp-session-id")
        .or_else(|| headers.get("mcp-session-id"))
        .cloned();
    let permits = Permits(permit);
    let allowlist = connection.tool_allowlist.clone();
    let body = match kind {
        BodyKind::Empty => {
            drop(permits);
            Body::empty()
        }
        BodyKind::Json if allowlist.is_some() && !tools_list.is_empty() => {
            let read = read_capped(response.bytes_stream(), MAX_FILTERED_BODY).await;
            drop(permits);
            let bytes = match read {
                Ok(bytes) => bytes,
                Err(ReadError::TooLarge) => {
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_too_large",
                        format!("connection {} answered with more than 8 MiB", connection.label),
                    );
                }
                Err(ReadError::Failed) => {
                    return refuse(
                        StatusCode::BAD_GATEWAY,
                        "upstream_unreachable",
                        format!("connection {} stopped answering", connection.label),
                    );
                }
            };
            let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
                return refuse(
                    StatusCode::BAD_GATEWAY,
                    "upstream_invalid",
                    format!("connection {} answered with JSON that does not parse", connection.label),
                );
            };
            jsonrpc::filter_tools_lists(&mut value, &tools_list, allowlist.as_deref().unwrap_or_default());
            Body::from(serde_json::to_vec(&value).expect("a JSON value serialises"))
        }
        BodyKind::Json => Body::from_stream(passthrough(response, permits, connection.id.clone())),
        BodyKind::EventStream => {
            let answerer = Answerer {
                state: state.clone(),
                principal,
                slug: slug.clone(),
                url: upstream.url.clone(),
                internal_network: upstream.internal_network,
                session_id,
                protocol_version: headers.get("mcp-protocol-version").cloned(),
                connection_id: connection.id.clone(),
            };
            Body::from_stream(events(response, permits, allowlist, tools_list, answerer))
        }
    };
    let mut answer = Response::new(body);
    *answer.status_mut() = status;
    *answer.headers_mut() = out;
    answer
}

fn body_too_large() -> Response {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        "a request body is at most 4 MiB",
    )
}

/// The permit a response holds until its body ends or is dropped.
struct Permits(#[allow(dead_code)] Permit);

/// What an upstream answered with, as the proxy passes it on.
enum BodyKind {
    /// No body at all.
    Empty,
    Json,
    EventStream,
}

/// Only JSON and event streams pass, uncompressed (gateway spec §5.2); a
/// body-less answer (202, 204, or `Content-Length: 0`) has no type to
/// judge (plan 8d decision 4). One `Content-Type` at most: a client could
/// read two otherwise than the gateway (the review's B1).
fn body_kind(response: &reqwest::Response) -> Result<BodyKind, &'static str> {
    let headers = response.headers();
    if headers.get_all(header::CONTENT_TYPE).iter().count() > 1 {
        return Err("more than one content type");
    }
    if headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err("a compressed body");
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().trim().to_ascii_lowercase());
    let bodiless = matches!(response.status(), StatusCode::ACCEPTED | StatusCode::NO_CONTENT)
        || response.content_length() == Some(0);
    match content_type.as_deref() {
        Some("application/json") => Ok(BodyKind::Json),
        Some("text/event-stream") => Ok(BodyKind::EventStream),
        None if bodiless => Ok(BodyKind::Empty),
        None => Err("a body of no content type"),
        Some(_) => Err("a content type other than JSON or an event stream"),
    }
}

/// The upstream's body as it comes, chunk by chunk, holding `permits`. A
/// failure mid-body ends it; its error carries no URL.
fn passthrough(
    response: reqwest::Response,
    permits: Permits,
    connection_id: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    response.bytes_stream().map(move |chunk| {
        let _held = &permits;
        chunk.map_err(|err| {
            let err = err.without_url();
            tracing::debug!(connection_id = %connection_id, error = %err, "gateway proxy: the upstream body failed");
            std::io::Error::other(err)
        })
    })
}

/// Answers server-to-client requests the gateway refuses (gateway spec
/// §5.6), on the stream's upstream session. It keeps no client and no
/// credential: each answer reads the connection again, as a request does
/// (plan 8b-ii's obligation: nothing cached across a `PATCH`).
struct Answerer {
    state: ProxyState,
    principal: Principal,
    slug: String,
    /// Where the stream's request went, and under which allowance.
    url: Url,
    internal_network: bool,
    session_id: Option<HeaderValue>,
    protocol_version: Option<HeaderValue>,
    connection_id: String,
}

impl Answerer {
    /// Post `answer` upstream, in the background. Skipped if the
    /// connection is at its request cap, or is no longer the one the
    /// stream came from: out of the principal's scope, at another URL or
    /// allowance, or without its credential.
    fn send(&self, answer: &Value) {
        let Ok(permit) = self.state.limits.requests.try_acquire(&self.connection_id) else {
            tracing::warn!(connection_id = %self.connection_id, "gateway proxy: a refused server request left unanswered, busy");
            return;
        };
        let current = match self.state.mounts.connection(&self.principal, &self.slug) {
            Ok(Some(connection)) if connection.id == self.connection_id => upstream(&self.state, &connection),
            Ok(_) => Ok(None),
            Err(err) => Err(err),
        };
        let auth = match current {
            Ok(Some(now)) if now.url == self.url && now.internal_network == self.internal_network => now.auth,
            Ok(_) => {
                tracing::warn!(connection_id = %self.connection_id, "gateway proxy: a refused server request left unanswered, the connection changed");
                return;
            }
            Err(err) => {
                tracing::error!(connection_id = %self.connection_id, error = %err, "gateway proxy: a refused server request left unanswered");
                return;
            }
        };
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(header::ACCEPT, HeaderValue::from_static(DEFAULT_ACCEPT));
        headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        if let Some(id) = &self.session_id {
            headers.insert("mcp-session-id", id.clone());
        }
        if let Some(version) = &self.protocol_version {
            headers.insert("mcp-protocol-version", version.clone());
        }
        if let UpstreamAuth::Static { header, value } = &auth {
            headers.insert(header.clone(), value.clone());
        }
        let mut request = reqwest::Request::new(Method::POST, self.url.clone());
        *request.headers_mut() = headers;
        *request.body_mut() = Some(serde_json::to_vec(answer).expect("a JSON value serialises").into());
        let client = egress_client(&self.state.egress, self.internal_network);
        let connection_id = self.connection_id.clone();
        tokio::spawn(async move {
            let _permit = permit;
            match client.send(request).await {
                Ok(response) => {
                    tracing::debug!(connection_id = %connection_id, status = response.status().as_u16(), "gateway proxy: a refused server request answered")
                }
                Err(err) => {
                    tracing::debug!(connection_id = %connection_id, error = %err, "gateway proxy: a refused server request's answer failed")
                }
            }
        });
    }
}

/// The upstream's event stream, event by event: each complete event goes
/// on at once, rewritten or dropped where a rule says so; a partial one
/// waits for its end. An event over 8 MiB, or a failure, ends the stream.
/// Fail closed (the review's B2): a byte-order mark at the start is
/// dropped, as a client's parser would; an event whose data is not JSON,
/// has a key twice, or has a line that starts with a mark, is dropped, since the gateway
/// cannot read what a client might; and a last event the stream never ends
/// is dropped, as a client drops it.
fn events(
    response: reqwest::Response,
    permits: Permits,
    allowlist: Option<Vec<String>>,
    ids: Vec<Value>,
    answerer: Answerer,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    struct State<S> {
        upstream: S,
        pending: Vec<u8>,
        /// Where in `pending` to go on looking for an event's end: the
        /// start of its last line seen (the review's O2).
        resume: usize,
        /// Whether a byte-order mark at the start was looked for.
        started: bool,
        done: bool,
        _permits: Permits,
        allowlist: Option<Vec<String>>,
        ids: Vec<Value>,
        answerer: Answerer,
    }
    let state = State {
        upstream: response.bytes_stream(),
        pending: Vec::new(),
        resume: 0,
        started: false,
        done: false,
        _permits: permits,
        allowlist,
        ids,
        answerer,
    };
    futures::stream::unfold(state, |mut state| async move {
        loop {
            if state.done {
                return None;
            }
            match state.upstream.next().await {
                None => {
                    state.done = true;
                    if !state.pending.is_empty() {
                        tracing::debug!(connection_id = %state.answerer.connection_id, "gateway proxy: an unended last event dropped");
                    }
                    return None;
                }
                Some(Err(err)) => {
                    state.done = true;
                    let err = err.without_url();
                    tracing::debug!(connection_id = %state.answerer.connection_id, error = %err, "gateway proxy: the upstream stream failed");
                    return Some((Err(std::io::Error::other(err)), state));
                }
                Some(Ok(chunk)) => {
                    state.pending.extend_from_slice(&chunk);
                    if !state.started {
                        if BOM.starts_with(&state.pending) {
                            // A mark still arriving, or none yet.
                            continue;
                        }
                        state.started = true;
                        if state.pending.starts_with(BOM) {
                            state.pending.drain(..BOM.len());
                        }
                    }
                    let mut out = Vec::new();
                    let mut answers = Vec::new();
                    let mut start = 0;
                    loop {
                        let end = match jsonrpc::event_end(&state.pending[start..], state.resume) {
                            Ok(end) => end,
                            Err(resume) => {
                                state.resume = resume;
                                break;
                            }
                        };
                        state.resume = 0;
                        let event = &state.pending[start..start + end];
                        match jsonrpc::rewrite_event(event, &state.ids, state.allowlist.as_deref(), &mut answers) {
                            EventOutcome::Unchanged => out.extend_from_slice(event),
                            EventOutcome::Rewritten(bytes) => out.extend_from_slice(&bytes),
                            EventOutcome::Dropped => {}
                            EventOutcome::Unreadable => {
                                tracing::warn!(connection_id = %state.answerer.connection_id, "gateway proxy: an event the gateway cannot read dropped");
                            }
                        }
                        start += end;
                    }
                    state.pending.drain(..start);
                    for answer in &answers {
                        tracing::info!(connection_id = %state.answerer.connection_id, "gateway proxy: a server request for a capability not forwarded refused");
                        state.answerer.send(answer);
                    }
                    if state.pending.len() > MAX_FILTERED_BODY {
                        state.done = true;
                        tracing::warn!(connection_id = %state.answerer.connection_id, "gateway proxy: an event over 8 MiB, the stream ended");
                        return Some((Err(std::io::Error::other("an event over 8 MiB")), state));
                    }
                    if !out.is_empty() {
                        return Some((Ok(Bytes::from(out)), state));
                    }
                }
            }
        }
    })
}
```

- [ ] **Step 5: Run them to see them pass**

Run: `cargo test -p hennery-gateway --locked`
Expected: PASS.

- [ ] **Step 6: Revert-probes**

- `mark-ok-call` (`proxy.rs`): the proxy never calls `mark_ok` → `live_traffic_sets_ok_on_a_2xx_only` fails.
- `mark-needs-auth-call` (`proxy.rs`): the proxy never calls `mark_needs_auth` → `an_upstream_401_is_502_upstream_auth_and_needs_auth` fails.
- `requests-limiter` (`proxy.rs`): the request permit taken from a fresh, unbounded `Limiter` → `requests_past_the_cap_are_503` fails.
- `streams-limiter` (`proxy.rs`): the stream permit taken from a fresh, unbounded `Limiter` → `get_streams_are_forwarded_and_capped` fails.
- `permit-held-by-body` (`proxy.rs`): `passthrough`'s body no longer holds the permits (`let _held = &permits;` dropped) → `requests_past_the_cap_are_503` fails.
- `stream-permit-held-by-body` (`proxy.rs`): the event stream's body drops the permits at once → `get_streams_are_forwarded_and_capped` fails.
- `body-cap` (`proxy.rs`): `read_capped` never refuses (`if false && …`) → `a_body_over_4_mib_is_413` fails.
- `no-authorization-up` (`proxy.rs`): `authorization` added to the forwarded request headers (a `none` connection) → `a_none_connection_sends_no_credential` fails.
- `no-authorization-up-custom-header` (`proxy.rs`): the same, for a token under `X-API-Key` → `a_static_token_under_its_own_header_and_no_authorization_upstream` fails.
- `credential-header` (`proxy.rs`): `upstream_headers` never inserts the credential → `a_request_goes_up_with_the_credential_and_the_allowed_headers_only` fails.
- `response-headers` (`proxy.rs`): `set-cookie` and `www-authenticate` added to the forwarded response headers → `a_request_goes_up_with_the_credential_and_the_allowed_headers_only` fails.
- `nosniff` (`proxy.rs`): the `nosniff` layer removed from the router → `out_of_scope_is_one_404` fails.
- `accept-default` (`proxy.rs`): no default `Accept` → `a_none_connection_sends_no_credential` fails.
- `accept-encoding-identity` (`proxy.rs`): no `Accept-Encoding: identity` → `a_request_goes_up_with_the_credential_and_the_allowed_headers_only` fails.
- `content-type-other` (`proxy.rs`): another content type taken as JSON → `only_json_and_event_streams_pass` fails.
- `content-type-none-with-body` (`proxy.rs`): a typeless body taken as JSON → `only_json_and_event_streams_pass` fails.
- `content-encoding` (`proxy.rs`): a compressed body passed (`.any(|v| v.is_empty())`) → `only_json_and_event_streams_pass` fails.
- `bodiless-pass` (`proxy.rs`): the body-less case removed (a 202 without a type becomes 502) → `only_json_and_event_streams_pass` fails.
- `redirect` (`proxy.rs`): a 3xx passed on (`if false && status.is_redirection()`) → `a_redirect_is_502_and_never_followed` fails.
- `401-held` (`proxy.rs`): a 401 passed on (`if false && status == UNAUTHORIZED`) → `an_upstream_401_is_502_upstream_auth_and_needs_auth` fails.
- `static-without-token` (`proxy.rs`): a static connection without a token sent with no credential → `a_static_connection_without_a_token_is_502_and_sends_nothing` fails.
- `egress-from-stored-flag` (`proxy.rs`): `egress_client` always internal (`internal_network || true`) → `the_egress_allowance_is_the_connection_s_stored_flag` fails.
- `json-tools-list-filter` (`proxy.rs`): the JSON `tools/list` answer's filter call removed → `tools_list_is_filtered_in_json_batches_and_event_streams` fails.
- `filtered-cap` (`proxy.rs`): the filtered read uncapped (`usize::MAX`) → `a_filtered_tools_list_over_8_mib_is_502` fails.
- `event-cap` (`proxy.rs`): the 8 MiB event cap never checked → `an_event_over_8_mib_ends_the_stream` fails.
- `answer-server-request` (`proxy.rs`): `state.answerer.send(answer)` removed → `a_server_request_for_sampling_is_answered_and_not_passed_on` fails.
- `drop-server-request` (`proxy.rs`): a dropped event passed on instead → `a_server_request_for_sampling_is_answered_and_not_passed_on` fails.
- `stream-json-unbuffered` (`proxy.rs`): a JSON answer read whole before it is sent (a buffering proxy) → `the_first_chunk_arrives_before_the_upstream_finishes` fails.
- `stream-sse-unbuffered` (`proxy.rs`): an event stream read whole before it is sent → `the_first_chunk_arrives_before_the_upstream_finishes` fails.
- `unreachable-log-origin` (`proxy.rs`): the unreachable warning logs `upstream.url` whole → `no_token_credential_or_url_secret_is_logged_or_answered` fails.
- `tools-call-blocked` (`jsonrpc.rs`): a refused `tools/call` not marked blocked → `a_tools_call_outside_the_allowlist_never_reaches_the_upstream` fails.
- `initialize-strip` (`jsonrpc.rs`): `initialize` not stripped → `initialize_goes_up_without_the_capabilities_not_forwarded` fails.
- `duplicate-keys` (`jsonrpc.rs`): the duplicate-key check skipped (`if false && …`) → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `sse-tools-list-filter` (`jsonrpc.rs`): an event's `tools/list` not filtered → `tools_list_is_filtered_in_json_batches_and_event_streams` fails.
- `batch-element-by-element` (`jsonrpc.rs`): a batch filtered as nothing (`Value::Array(_) => Vec::new()`) → `tools_list_is_filtered_in_json_batches_and_event_streams` fails.
- `empty-is-array` (`jsonrpc.rs`): a `tools: null` result left without `tools` (`_ => continue`) → `jsonrpc::tests::a_tools_list_response_is_filtered_and_an_empty_one_is_an_array` fails.
- `b1-one-type-down` (`proxy.rs`): the JSON answer's type taken from the upstream's header → `the_answer_s_type_is_the_one_the_gateway_judged` fails.
- `b1-two-types` (`proxy.rs`): two `Content-Type` headers accepted → `the_answer_s_type_is_the_one_the_gateway_judged` fails.
- `b2-bom` (`proxy.rs`): the leading byte-order mark kept → `an_event_stream_fails_closed_on_what_it_cannot_read` fails.
- `b2-unreadable` (`jsonrpc.rs`): an event whose data is not JSON passed as it came → `an_event_stream_fails_closed_on_what_it_cannot_read` fails.
- `b2-unended-tail` (`proxy.rs`): the stream's unended last event sent at its end → `an_event_stream_fails_closed_on_what_it_cannot_read` fails.
- `o1-same-id` (`jsonrpc.rs`): ids compared as JSON values only (`1` is not `1.0`) → `tools_list_is_filtered_in_json_batches_and_event_streams` fails.
- `o3-body-timeout` (`proxy.rs`): the body read's deadline an hour → `a_body_that_does_not_arrive_in_time_is_408` fails.
- `o3-get-no-request-permit` (`proxy.rs`): a `GET` takes a request permit as well → `a_get_stream_takes_no_request_permit` fails.
- `o5-bodiless-sets-nothing` (`proxy.rs`): a body-less 2xx sets `ok` → `live_traffic_sets_ok_on_a_2xx_only` fails.
- `o6-type-up` (`proxy.rs`): the client's `Content-Type` forwarded beside the gateway's → `a_none_connection_sends_no_credential` fails.
- `o7-deeper-404` (`proxy.rs`): the `/mcp/{slug}/{*rest}` route removed → `out_of_scope_is_one_404` fails.
- `bom-line` (`jsonrpc.rs`): an event with a line that starts with a byte-order mark read as usual → `an_event_stream_fails_closed_on_what_it_cannot_read` fails.
- `t2r-head-404` (`proxy.rs`): the explicit `HEAD` route removed (axum hands `HEAD` to the `GET` handler) → `out_of_scope_is_one_404` fails.
- `t2r-method-fallback` (`proxy.rs`): the route's method fallback removed (axum's empty 405) → `out_of_scope_is_one_404` fails.
- `t2r-trailing-slash-404` (`proxy.rs`): the `/mcp/{slug}/` route removed → `out_of_scope_is_one_404` fails.
- `t2r-no-store` (`proxy.rs`): the `no-store` layer removed from the router → `a_request_goes_up_with_the_credential_and_the_allowed_headers_only` fails.
- `t2r-empty-json-sets-nothing` (`proxy.rs`): an empty body under a JSON type sets `ok` → `live_traffic_sets_ok_on_a_2xx_only` fails.
- `t2r-sse-duplicate-keys` (`jsonrpc.rs`): an event's data read without the duplicate-key check (`Value` only) → `an_event_stream_fails_closed_on_what_it_cannot_read` fails.
- `t2r-head-timeout` (`proxy.rs`): the request's head timeout not set → `an_upstream_that_never_answers_is_502_after_the_head_timeout` fails.
- `t2r-answerer-cap` (`proxy.rs`): the answerer's permit taken from a fresh, unbounded `Limiter` → `a_refused_server_request_s_answer_takes_the_request_s_session_and_a_permit` fails.
- `t2r-answerer-session-fallback` (`proxy.rs`): the answer's fallback to the request's `Mcp-Session-Id` dropped → `a_refused_server_request_s_answer_takes_the_request_s_session_and_a_permit` fails.
- `host-not-forwarded` (`proxy.rs`): the client's `Host` forwarded (`host` on the request allowlist) → `a_request_goes_up_with_the_credential_and_the_allowed_headers_only` fails.
- `egress-plain-http-lan` (`proxy.rs`): `egress_client` always internal, for plain `http` to a LAN address → `an_unmarked_connection_is_refused_plain_http_to_a_lan_address` fails.
- `wb-respelt-up` (`jsonrpc.rs`): the request's respelt-key check skipped (`if false && …`) → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `wb-respelt-message` (`jsonrpc.rs`): a message's own keys not checked for another spelling → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `wb-respelt-name` (`jsonrpc.rs`): a `tools/call`'s `params.name` not checked for another spelling → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `wb-respelt-capabilities-key` (`jsonrpc.rs`): an `initialize`'s `params.capabilities` not checked for another spelling → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `wb-respelt-fold` (`jsonrpc.rs`): `ſ` not folded to `s` → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `wb-respelt-down` (`jsonrpc.rs`): an event's respelt-key check skipped → `an_event_stream_fails_closed_on_what_it_cannot_read` fails.
- `wb-slug-utf8` (`proxy.rs`): a slug that is not UTF-8 answered 400 → `out_of_scope_is_one_404` fails.
- `wb-answerer-url` (`proxy.rs`): the answerer sends after the connection's URL moved → `a_refused_server_request_after_its_connection_changed_is_left_unanswered` fails.
- `wb-answerer-flag` (`proxy.rs`): the answerer sends after the connection's internal marking went → `a_refused_server_request_after_its_connection_changed_is_left_unanswered` fails.
- `wb-answerer-same-id` (`proxy.rs`): the answerer sends for another connection under the same slug → `a_refused_server_request_after_its_connection_changed_is_left_unanswered` fails.
- `wb-respelt-separators` (`jsonrpc.rs`): `_` and `-` kept when folding a key → `a_body_that_is_not_json_or_has_a_key_twice_is_400` fails.
- `oc-status-passed-through` (`proxy.rs`): every upstream status passed on as 200 → `live_traffic_sets_ok_on_a_2xx_only` fails.
- `oc-body-declared` (`proxy.rs`): the declared `Content-Length` not checked → `a_body_declared_over_4_mib_is_413_before_it_is_read` fails.
- `oc-upstream-invalid` (`proxy.rs`): a filtered `tools/list` that does not parse read as `{}` → `a_filtered_tools_list_that_does_not_parse_is_502` fails.
- `oc-nothing-to-answer` (`proxy.rs`): a body with nothing to answer is 200, not 202 → `a_tools_call_outside_the_allowlist_never_reaches_the_upstream` fails.
- `oc-egress-internal-arm` (`proxy.rs`): `egress_client` never internal → `the_egress_allowance_is_the_connection_s_stored_flag` fails.
- `oc-unreachable` (`proxy.rs`): an unreached upstream answered with another code → `an_upstream_that_never_answers_is_502_after_the_head_timeout` fails.
- `oc-internal` (`proxy.rs`): an internal error answered 502 → `a_connection_the_store_holds_damaged_is_500_internal` fails.

Measured inert, kept: `passthrough-without-url` and `events-without-url` (removing `without_url()` from the body stream's error) leave the hygiene test passing, while its positive control shows both lines are logged: reqwest 0.12's body errors carry no URL. The calls stay, as plan 8b's contract asks, for a reqwest that adds one.

- [ ] **Step 7: Load**

Run four copies of the proxy test binary at once, three times (`for i in 1 2 3; do for j in 1 2 3 4; do target/debug/deps/proxy-* & done; wait; done`).
Expected: every run passes.

- [ ] **Step 8: The full checks, then commit**

```bash
git add Cargo.lock crates/hennery-gateway
git commit -m "feat(gateway): the proxy, /mcp/<slug>, for static and none credentials"
```

### Task 3: The collector serves the proxy

**Files:**
- Modify: `crates/hennery/src/main.rs`, `crates/hennery/tests/cli.rs`

**Anchors:** `run_collector`'s `let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;` and its `serve_all` call (8a); `cli.rs`'s `collector_on`, `stop`, `RemoveDir`, and `refused_start` (8a).

- [ ] **Step 1: Write the failing test**

In `crates/hennery/tests/cli.rs`, replace:

```rust
    assert!(!data.join("master.key").exists(), "a new key was made");
```

with:

```rust
    assert!(!data.join("master.key").exists(), "a new key was made");
}

/// Plan 8d (lane L8, kernel spec §3.3): the collector serves the gateway's
/// proxy at `/mcp/<slug>`, outside the operator's routes. A request with a
/// foreign `Origin`, a cross-site `Sec-Fetch-Site`, no cookie and no JSON
/// type, before setup even, gets the proxy's own 404, not the browser
/// rules' 403 or 415, with `nosniff` and nothing compressed.
#[test]
fn the_collector_serves_the_mcp_proxy_outside_the_operator_s_routes() {
    let dir = scratch_dir("mcp-proxy");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("collector.log"));
    let token = format!("hnry_session_{}", "0".repeat(64));
    for (method, body) in [
        ("POST", r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
        ("GET", ""),
        ("DELETE", ""),
    ] {
        let mut stream = TcpStream::connect(&listen).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        write!(
            stream,
            "{method} /mcp/linear HTTP/1.1\r\nHost: {listen}\r\nOrigin: https://evil.example\r\nSec-Fetch-Site: cross-site\r\nAuthorization: Bearer {token}\r\nContent-Type: text/plain\r\nAccept-Encoding: gzip, br\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let head = head.to_ascii_lowercase();
        assert!(head.starts_with("http/1.1 404"), "{method}: {response}");
        assert!(head.contains("x-content-type-options: nosniff"), "{method}: {head}");
        // A guard for later: no compression layer exists yet, and one added
        // around the merged router would compress this body (kernel spec §7).
        assert!(!head.contains("content-encoding"), "{method}: {head}");
        assert!(body.contains(r#""code":"not_found""#), "{method}: {body}");
    }
    stop(&mut collector);
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p hennery --locked --test cli the_collector_serves_the_mcp_proxy`
Expected: FAIL: the collector answers `/mcp/linear` with axum's 404 without a body (or 405), not the proxy's `not_found`.

- [ ] **Step 3: Commit the test**

```bash
git add crates/hennery/tests/cli.rs
git commit -m "test(cli): the collector serves the MCP proxy outside the operator's routes"
```

- [ ] **Step 4: Serve it**

In `crates/hennery/src/main.rs`, replace:

```rust
    let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
```

with:

```rust
    let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
    // The gateway's proxy (plan 8d), `/mcp/<slug>`: bearer tokens, beside
    // the operator's routes and outside them (lane L8), sending only
    // through the kernel's egress policy, the collector's one `Egress`
    // above, shared with Web Push.
    let proxy = hennery_gateway::proxy::ProxyState::full(
        std::sync::Arc::new(hennery_gateway::scope::ProxyStore::open(&db)?),
        gateway.store.clone(),
        gateway.key.clone(),
        egress.clone(),
        hennery_gateway::proxy::Limits::default(),
    );
```

In `crates/hennery/src/main.rs`, replace:

```rust
        hennery_sessions::router(state.clone()).merge(hennery_gateway::api::router(gateway)),
```

with:

```rust
        hennery_sessions::router(state.clone())
            .merge(hennery_gateway::api::router(gateway))
            .merge(hennery_gateway::proxy::router(proxy)),
```

- [ ] **Step 5: Run it to see it pass; revert-probe**

Run: `cargo test -p hennery --locked --test cli the_collector_serves_the_mcp_proxy`
Expected: PASS. Revert-probe: drop the `.merge(hennery_gateway::proxy::router(proxy))` line (and the `proxy` binding it uses): the test fails.

- [ ] **Step 6: The full checks, then commit**

```bash
git add crates/hennery/src/main.rs
git commit -m "feat(cli): the collector serves the gateway's proxy"
```

### Task 4: The gateway spec's §3.1, §5 and §7

**Files:**
- Modify: `docs/specs/2026-09-26-mcp-gateway-design.md`; and, where 8d makes them stale, `docs/specs/2026-09-26-kernel-design.md` (§7.1) and `docs/specs/2026-09-25-hennery-architecture-design.md` (§10.1)

- [ ] **Step 1: Write back the decisions**

In `docs/specs/2026-09-25-hennery-architecture-design.md`, replace:

```markdown
    reach the client before the upstream finishes writing. A buffering proxy
```

with:

```markdown
    reach the client before the upstream finishes writing (for an event
    stream, every chunk that ends an event; gateway §5.3). A buffering proxy
```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

```markdown
  (stricter than gateway §5.7's idle streams): a count of permits per caller
  key, refused at once past its cap (the proxy's 503).
```

with:

```markdown
  (gateway §5.7): a count of permits per caller key, refused at once past
  its cap (the proxy's 503).
```

In `docs/specs/2026-09-26-kernel-design.md`, replace:

```markdown
10b-ii); the gateway's proxy and OAuth will.
```

with:

```markdown
10b-ii), and the gateway's proxy through the client its connection's marking
chooses (plan 8d), both from the collector's one `Egress`; OAuth will.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  session_id PK, owner_id, host_id, hat_id, token_hash,
```

with:

```markdown
  session_id PK, owner_id, host_id, hat_id, token_hash UNIQUE,
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  `start_session` / `resume_session` frame for that session's host.
```

with:

```markdown
  `start_session` / `resume_session` frame for that session's host.
- A session token is `hnry_session_` and 64 lowercase hexadecimal digits, a
  prefix a secret scanner can match; anything else is not looked up (plan
  8d). Standalone tokens get a prefix of their own (plan 8g).
- The gateway mints and revokes inside the sessions store's own
  transactions (`tokens::mint_in`, `revoke_in`, `revoke_host_in`, each
  taking the caller's `rusqlite::Transaction`), so a transition that rolls
  back leaves no token minted or revoked. A mint replaces the session's
  row: the previous token no longer resolves. A hat purge deletes its
  tokens, revoked ones too (`purge_hat_in`).
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  hat all return **404** (not 403: do not confirm existence).
```

with:

```markdown
  hat all return **404** (not 403: do not confirm existence). So do a
  missing, malformed or repeated `Authorization`, a superseded token, a
  token of a revoked host and an unknown slug: one body for all of them, and
  never 401 (G-17). A revoked host's tokens and mounts are out of scope
  whether or not its tokens were revoked.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown

### 5.2 Forwarding
```

with:

```markdown

Every answer the proxy makes itself is an `ApiError`, `{code, message}`, as
every other route's: 404 `not_found`, 400 `invalid_request`, 408
`request_timeout` (a body has 30 s to arrive), 413 `body_too_large`, 503
`busy`, 502 `upstream_auth`, `upstream_unreachable`, `upstream_redirect`,
`upstream_content_type`, `upstream_too_large` or `upstream_invalid`, 500
`internal`. None names more of the upstream URL than its connection's label.
A JSON-RPC error inside an MCP answer (§5.5, §5.6) stays JSON-RPC. Anything
deeper under a slug (`/mcp/<slug>/…`, a bare trailing slash too), a slug
that is not UTF-8, and any other method (`HEAD` included) is the same 404,
and nothing goes up.

### 5.2 Forwarding
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
- **Response headers forwarded:** `Content-Type`, `Mcp-Session-Id`,
  `Cache-Control`. Everything else is dropped, including `Set-Cookie` and
  `WWW-Authenticate`. *(G-15: the predecessor passed `Set-Cookie` through.)*
  The gateway always adds `X-Content-Type-Options: nosniff`, and forwards only
  `application/json` and `text/event-stream` bodies; any other upstream content
  type becomes a 502.
- Upstream redirects are never followed (§5.7).
```

with:

```markdown
  `Content-Type: application/json` goes up with a `POST` (the gateway checked
  the body is JSON), `Accept-Encoding: identity` is sent, and the upstream
  URL is the connection's as stored: the client's path beyond the slug and
  its query are not forwarded. A static credential is read in one statement
  with the URL, header and prefix it goes with.
- **A `POST` body must be JSON with no key twice in any object**, or it is
  400 `invalid_request` and nothing is sent: a parser upstream that keeps
  the first of two `"method"`s or `"name"`s would otherwise run what the
  allowlist never saw. Nor may it spell a key the gateway reads otherwise
  than exactly, folding ASCII case and `ſ` to `s` and ignoring `_` and `-`,
  as Go's `encoding/json` (v1 and v2) can match names: a message's
  `jsonrpc`, `id`, `method` and `params`, a `tools/call`'s `name`, an
  `initialize`'s `capabilities` and the capabilities not forwarded.
  `METHOD` or `Name` would be read upstream as what the allowlist never
  saw. The bytes go up as they came, except an
  `initialize` rewritten (§5.6). `GET` and `DELETE` bodies are neither read
  nor sent.
- **Response headers forwarded:** `Mcp-Session-Id`; `Content-Type` is the
  gateway's (below). `Cache-Control` is always `no-store`, the proxy's own
  answers' too: an answer to a request that carries a token is no shared
  cache's to keep, whatever the upstream says. Everything else is dropped,
  including `Set-Cookie` and `WWW-Authenticate`. *(G-15: the predecessor
  passed `Set-Cookie` through.)*
  The gateway always adds `X-Content-Type-Options: nosniff`, and forwards only
  `application/json` and `text/event-stream` bodies; any other upstream content
  type becomes a 502. So does a body without a content type, more than one
  `Content-Type`, and any `Content-Encoding` but `identity`. A body-less
  answer (202, 204, or `Content-Length: 0`) without a type passes, empty. The
  `Content-Type` sent down is the gateway's, the one type it judged the body
  by, without parameters: a parameter or a second header cannot make a
  client read as an event stream what the gateway passed as JSON.
- Upstream redirects are never followed (§5.7): a 3xx is 502
  `upstream_redirect`, its `Location` never forwarded.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
- Responses are streamed chunk by chunk with no buffering and **no compression
  layer on the proxy route** (compression middleware delays SSE).
- **Guarantee:** the first chunk of an upstream response reaches the client
  before the upstream finishes writing. A test asserts it against an upstream
  that writes one chunk and then blocks; the test must fail within seconds,
  not hang, if the proxy buffers.
- Exception: a `tools/list` response for a connection with an allowlist is
  read fully (cap 8 MiB, error if exceeded — never truncated) and rewritten.

### 5.4 Upstream 401

- Every response status other than 401 is committed and streamed immediately.
- A 401 is held: static or `none` credential, or OAuth without a refresh
  token → **502** `{"error": "upstream_auth", "message": "connection <label>
```

with:

```markdown
- JSON responses are streamed chunk by chunk with no buffering, event
  streams event by event (below), and there is **no compression layer on the
  proxy route** (compression middleware delays SSE).
- **Guarantee:** the first chunk of an upstream response reaches the client
  before the upstream finishes writing. A test asserts it against an upstream
  that writes one chunk and then blocks; the test must fail within seconds,
  not hang, if the proxy buffers. For an event stream the guarantee holds
  for every chunk that ends an event; a partial event waits for its end.
- Exception: a `tools/list` response in JSON for a connection with an
  allowlist is read fully (cap 8 MiB, error if exceeded — never truncated)
  and rewritten; in an event stream, its event is.
- An event stream is passed on **event by event**: every complete event at
  once, a partial one held until its end, so an event can be rewritten or
  dropped (§5.5, §5.6). One event is at most 8 MiB; past that the stream
  ends. It fails closed: a byte-order mark at its start is dropped, as a
  client's parser would; an event whose data is not JSON (to serde_json: a
  lone surrogate, say), has a key twice in an object, spells a key the
  gateway reads otherwise (§5.2), or has a line that starts with a
  byte-order mark, is dropped, since a client's parser might read what the
  gateway cannot; a last event the stream never ends is
  dropped (a final lone `\r` included). A rewritten event keeps its other
  fields (`id:`, `event:`) and gets one `data:` line.
  Events without data (comments, pings) pass byte for byte, and invalid
  UTF-8 in an event no rule touches goes on as it came, as clients decode
  it with replacement characters.

### 5.4 Upstream 401

- Every response status other than 401 is committed and streamed immediately.
- A 401 is held: static or `none` credential, or OAuth without a refresh
  token → **502** `{"code": "upstream_auth", "message": "connection <label>
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  upstream headers.
```

with:

```markdown
  upstream headers.
- A `static` connection without a token, or of a kind the proxy does not take
  yet, sends nothing: 502 `upstream_auth`.
- A 401 for a static or `none` connection sets it `needs_auth` (§7).
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  tools; a direct call still executed.)*
```

with:

```markdown
  tools; a direct call still executed.)* A call whose `params.name` is not a
  string is outside it. A batch holding one is answered whole by the
  gateway and nothing in it is sent: that call `-32602`, every other request
  in it `-32600` ("batch refused"); notifications get nothing, and a body
  with nothing to answer is 202. A response's id matches a `tools/list`
  request's if equal or the same number however written (`1`, `1.0`).
- **The filter hides; the `tools/call` refusal enforces.** The filter
  knows only the ids of the `tools/list` requests of the same exchange, so
  an unfiltered list can still reach a client: an id answered as another
  type (`"1"` for `1`), an interrupted `tools/list` replayed on a `GET`
  with `Last-Event-ID`, a response the upstream sends on another stream,
  or one whose `result`, `tools` or a tool's `name` is spelt otherwise or
  twice, for a client that reads it so. A tool seen that way still cannot
  be called.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
code passed the client's `initialize` through verbatim.)*
```

with:

```markdown
code passed the client's `initialize` through verbatim.)* The requests are
`sampling/createMessage`, `elicitation/create` and `roots/list`; each is
answered `-32601` with a `POST` on the same upstream session
(`Mcp-Session-Id`, the answer's or else the request's) and credential, in the
background, and it is not passed on: its event is dropped or, in a batch, its
element. Each answer reads the connection again, as a request does, and is
not sent if the connection left the token's scope or changed its URL or
internal marking since the stream opened. A notification of those names
passes. At the connection's request cap (§5.7) the answer is skipped and
logged. These requests are refused in
event streams only: a JSON body answering a `POST` is that request's
response, so inside one they pass in v1.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
- Per connection, a cap on concurrent upstream requests and on idle streaming
  responses; beyond it the proxy answers 503.
```

with:

```markdown
- Per connection, a cap on concurrent upstream requests and on open streaming
  responses, idle or not; beyond it the proxy answers 503.
  - `POST` and `DELETE` requests in flight: 64, each held from before its
    body is read until its answer's body ends or the client goes, a `POST`
    answered with an event stream included. A body has 30 s to arrive.
  - Open `GET` streams, the server-to-client channel: 32, counted whether
    idle or not (kernel §7.1's limiter); no idle timeout. A `GET` takes no
    request permit.
  - The response head must arrive within 300 s (a long `tools/call` may
    answer in JSON only when done).
- The egress client is chosen at every request from the connection's
  stored `internal_network` flag, read with its URL from the same row, never
  from anything in the request; it applies the rules above (kernel §7.1).
  The stored scheme and authority are sent verbatim, and the client's `Host`
  is never forwarded.
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
egress policy's scheme rule (kernel §7.1: `https`, or `http` to loopback);
which of the two holds is settled by the proxy plan (8d). Under a connection's
`internal_network` mark, egress sends plain `http` to internal addresses only
(kernel §7.1, plan 8b-ii). Non-public addresses
```

with:

```markdown
egress policy's scheme rule (§5.7, kernel §7.1) for an unmarked connection;
for a marked one the policy is the stricter, sending plain `http` to
internal addresses only (plan 8b-ii). Neither implies the other, and both
apply: the proxy sends every request through the egress policy (plan 8d),
so a saved URL the policy refuses is refused when used. Non-public addresses
```

In `docs/specs/2026-09-26-mcp-gateway-design.md`, replace:

```markdown
  `needs_auth`/`error` sets `ok`.
```

with:

```markdown
  `needs_auth`/`error` sets `ok` (and `not_connected`: a 2xx proves the
  upstream takes what was sent), if it has a JSON or event-stream body: a
  body-less 202, or an empty body under a type (`Content-Length: 0`),
  proves nothing. A 401 sets a
  static or `none` connection `needs_auth`. Either only if the connection
  still has the URL the request went to, and `ok` on a static one only if it
  still has a token; no other status changes it.
```

- [ ] **Step 2: Commit**

```bash
git add docs/specs/2026-09-26-mcp-gateway-design.md docs/specs/2026-09-26-kernel-design.md docs/specs/2026-09-25-hennery-architecture-design.md
git commit -m "docs(spec): write back plan 8d's proxy and session tokens"
```

## After this plan

**Open for the maintainer (the whole-branch review's finding 2, a product question):** scope is checked when a request arrives. A stream opened before a token's revoke, an unmount, a `PATCH` or a host's revoke keeps delivering what the upstream sends, for as long as it is open (a `GET` channel has no idle timeout); only the refused-request answers re-check (decision 14). Nothing mints a token before 8e, so no stream can exist to outlive anything, and this plan is not exposed. Should 8e end a token's open streams when it revokes it (a per-token cancellation the proxy's streams watch), or is a stream outliving its revoke accepted? The cases that matter are a host's revoke (a possibly compromised host still receiving) and a re-assignment (`reassign_hat`: an open stream keeps the old hat's connection).

**Obligations this plan hands on:**
- **Sessions (8e):**
  - call `tokens::mint_in` inside `create_session`'s and `request_resume`'s transactions, before `commit`, and put `SessionToken::expose()` only into that session's frame (`Authorization: Bearer …`);
  - call `revoke_in` at each of L4's revoke sites, and `revoke_host_in` in `revoke_host`, each in its own transaction, each with its revert-probe; a presumed park does not revoke. `reassign_hat` above all (the review's O8): a token's `hat_id` is fixed at mint, so a missed revoke there leaves the session reaching its old hat's connections until its next park;
  - build `servers_for`'s list: the connections of the session's hat mounted on its host, a query for `scope.rs` beside `MountPolicy::connection`, with the same joins (the hat, the mount, the host not revoked);
  - a session's delete: today a revoke; add a `forget_in(tx, session_id)` that deletes the row if the purge lane wants none left.
- **Purge (9, or whoever wires `on_hat_purged`, lane L6):** call `tokens::purge_hat_in` with 8a's `purge_hat`. Without it, the tokens' hat foreign key keeps the hat row, and the kernel's delete fails. That fails closed, never with a token left for a hat that is gone (`a_hat_purge_takes_its_tokens_and_then_the_hat_can_go`).
- **OAuth (8f):**
  - `refreshed` in `proxy.rs` is the 401 seam: return a fresh `UpstreamAuth` after a single-flight refresh, and the proxy retries once and then answers 502 and sets `needs_auth`;
  - `upstream()` grows the OAuth arms, which answer 502 `upstream_auth` until then;
  - the `Notifier` fires on the transitions `mark_ok` and `mark_needs_auth` report (both return whether the status changed);
  - the probe's `error` is the probe's to set, not live traffic's (decision 8);
  - the pre-designed `checked_at` (api-8e-8f): if live traffic moves it, throttle the write as `last_used_at` is (decision 12), not one per request.
- **Standalone (8g):** add `PrincipalKind::Client { client_id }`, resolved from `gw_clients` by its own token prefix, and the pins in `MountPolicy::connection`; the irrefutable `let PrincipalKind::Session { .. }` in `scope.rs` stops compiling until the new variant is handled.
- **Egress:** one per collector. Plan 10b-ii builds it in `main.rs` for Web Push, and the proxy takes a clone; OAuth (8f) clones the same one rather than a second, or the pools would split. 8f checks an authorization server's scheme itself (plan 8b-ii's obligation), since `InternalNetwork` now sends plain `http` to internal addresses.
- **Frontend (plan 4):** a connection's `status` now moves on live traffic (decision 8), and `status_note` says why it is `needs_auth`. The SPA must never route or serve HTML under `/mcp/*`: the proxy answers every path below a slug, and only the bare `/mcp` page is the view's (the review's O7).

**Left for 8a's successors** (the lane parent's ruling of 2026-10-02: what 8d needs of 8a's code is decision 15's lines, made in this branch; nothing else of 8a's handlers changes here):
- 8a's create and update do not call `egress::check_url` (plan 8b's hand-off): plan 8b-ii, or the next plan that touches those handlers, adds it, so a URL the proxy would refuse fails when saved;
- `purge_hat` could call `tokens::purge_hat_in` in its own transaction, once both are on `main`, rather than leaving it to the wiring.

**The review's Q1, decided by the lane parent (2026-10-02): §5.6 stands, the refusal applies to event streams only.** A server-to-client request (`sampling/createMessage` and the others) could also arrive inside a plain JSON answer; rmcp, Codex's client, hands such a body to its handler. But a JSON body answering a `POST` is that request's response, not a server-initiated request, so this plan keeps §5.6's "arriving in a response stream": a JSON answer streams through unread, and the first-chunk guarantee holds for JSON. The accepted risk: an upstream ignoring the stripped capabilities can still put such a request before a JSON-reading client. Reading every JSON answer whole, up to a cap, would close it at that cost; the `tools/list` path already reads its JSON whole.

**Operator items:** a live gate (GW §11) of one static-token connection through a real agent session once 8e lands; whether any vendor uses the `GET` channel for the server requests this plan refuses (GW §13 question 2).

---

_Generated with Claude AI — please review before distribution._
