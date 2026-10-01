# Operator authentication (plan 3b-i) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The development bearer token leaves the REST API; the operator signs in instead.
- On first start the collector writes a one-time setup link to a private file (it prints it only to a terminal). The token is in the link's fragment, so it never reaches a request line.
- Setup creates the one owner with an Argon2id password and a `public_url`, and signs them in.
- Every operator route then needs a server-side session behind an `HttpOnly`, `SameSite=Strict` cookie, and the `Origin` and `Sec-Fetch-Site` rules of kernel §3.3.
- Login is rate limited per address, with one password check per attempt.
- Minting a pairing code, revoking a host and revoking a session need a password check within the last five minutes (step-up). Ending a session ends the event streams it holds open.
- `hennery host join` also takes its code on standard input.

**Architecture:**
- **Kernel** (`hennery-kernel`):
  - `operator.rs`, the `Operator` store, on its own connection to `hennery.db`. It holds the owner, the password, the `public_url` setting, the in-memory setup token and the `setup-url` file, the `auth_sessions` rows, the login and step-up limiters, and a `watch` channel bumped whenever a session ends.
  - `auth_api.rs`: the HTTP handlers for `/api/setup` and `/api/auth/*`, with a 16 KiB body limit.
  - `setup_page.rs`: the static `/setup` page and its `/setup.js`, which read the token from the fragment.
  - `origin.rs`: the browser rules.
  - `auth.rs`: `operator_only` (browser rules, then the session cookie) and `require_step_up`.
  - `schema.rs`: the kernel's migrations, moved out of `hosts.rs`, now with a second one.
  - `auth.rs` also has `session_ended`, which the session stream waits on.
- **Wire** (`hennery-proto`): `SetupRequest`, `SetupResponse`, `LoginRequest`, `StepUpRequest` and `AuthSessionItem`. The three request types redact their password in `Debug`.
- **Collector** (`hennery-sessions`):
  - `AppState` gains `operator` and loses `token`.
  - The session API and the host routes sit behind `operator_only`. Minting and revoking a host also sit behind `require_step_up`.
  - The session stream ends when the operator's session that opened it ends.
  - Enrollment and the host WebSocket stay outside both.
- **Binary:** `--dev-token` is gone. `collector` announces the setup link once it listens. The `code` argument of `host join` becomes optional.
- **Testkit:** `hennery_testkit::operator_client(&Operator)` sets a test collector up and opens a session through the kernel API, so no test-only production path exists. The CLI tests sign in through `setup-url`, the way an operator would.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8, rusqlite 0.40, reqwest 0.12 (rustls). There are two new crates, pinned exactly in `[workspace.dependencies]`:
- `password-auth = "=1.0.0"`: Argon2id through `argon2` 0.5 and `blake2`. Stable; 1.1.0 is only a release candidate.
- `url = "=2.5.8"`: already in `Cargo.lock` through reqwest.

The kernel also takes `tokio` and `time` from the workspace. Everything is pure Rust, so the nix dev shell needs nothing new and `flake.nix` is unchanged. `argon2` and `blake2` are built with `opt-level = 3` in the dev profile (decision 5).

**Spec:** [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md), these sections:
- §1: one database. Every table has `owner_id`; this plan puts it on its new tables only (decision 1).
- §1.1: the kernel tables `owners`, `password_credentials`, `auth_sessions` and `settings`.
- §2: "Secrets are never accepted as CLI flags".
- §3.1: setup.
- §3.2: password login, sessions and the cookie. Passkeys are 3c.
- §3.3: the `Origin` rules per route class.
- §3.4: step-up.
- §8: the auth API.
- §10: the threat model.
- §11: setup-token and login tests, `Origin` per route class, and step-up.

It also relies on the umbrella spec [`2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md):
- §7.2: the bootstrap link;
- §7.3: login and the cookie;
- §7.4: every API call authenticated, and `owner_id` on every row;
- §7.5: `public_url` and TLS except on loopback.

It builds on the executed [host pairing plan](2026-10-01-host-pairing.md) (plan 3a). Read its "Execution status" and "After this plan" first. Its code wins over its task text. It also builds on PR #9, which shipped decision 14 (`IMMEDIATE` transactions) ahead of this plan. Every anchor below was taken from that code (`main` at `a005dfa`, which merged #9).

**Status:** executed 2026-10-02 (see "Execution status"). Amended after the security review of 2026-10-02: required amendments A1–A11, plus the optional hardening the coordinator took (see "Decisions"). Every code block below was built and tested in a scratch copy of `a005dfa`, and every block was generated from the scratch commits. The plan was then replayed from its own text, task by task, onto a fresh copy of `a005dfa`:
- each block applied exactly as "Reading the steps" says;
- after every task the tree matched the scratch commit byte for byte, `Cargo.lock` and the generated files included;
- after every task the replay ran fmt, both clippy runs (the second on the shipped binary, with test hooks off), the workspace tests and both codegen checks.

The replay ended with 431 tests, up from 393; execution ended with 439 (see "Execution status"). The timing-sensitive tests passed with four copies of their test binary running at once.

## Execution status (2026-10-02)

**Executed** on branch `feat/operator-auth`: one subagent per task, each followed by a review (Task 7's review was folded into the whole-branch review, as a small task); then a whole-branch security review of `a005dfa..55e00c3` ("with fixes") and a fix wave. The plan's decisions were confirmed by a stronger-model security review on the maintainer's behalf, which added amendments A1–A11; decision 14 shipped ahead of the plan as PR #9. Deviations found in review:

| Area | As built | Why |
|---|---|---|
| Hashing permit (T1) | `Operator.hashing` is an `Arc<Semaphore>`; `check_password` moves an owned permit into the blocking closure. Two unit tests pin the bound, one of them for an abandoned check. | A borrowed permit was freed when the caller's future was dropped, while its verify ran on: 15 verifies at once were reproduced |
| `SetupLink` (T3) | `Debug` is written by hand and prints the link as `…/setup#<redacted>` | The derived `Debug` printed the live token |
| Layer order (T5) | `the_browser_rules_run_before_the_session_check` pins that a cross-origin request without a cookie gets 403, not 401 | Swapping the two layers passed every other test |
| Self-revoke (T6) | Revoking the request's own session sends a single `Set-Cookie`, the clearing one: `require_operator` re-sends a slid cookie only if the handler set none and the session still exists. `revoke_session` matches only live rows, so an expired session answers 404. | Two `Set-Cookie`s left the browser holding the dead token; a revoke of a dead row answered 204 and bumped the ending generation |
| Locked-out step-up (T6) | `wrong_step_ups_lock_out_step_up_only` asserts that the refused step-up runs no password check (`verifications()` unchanged) | Only login's test pinned the no-verify path |
| Session cookies (final) | Every `hennery_session` cookie of a request is tried in order, at most `MAX_SESSION_COOKIES` (4), by `require_operator` and by logout; the first that authenticates is used, and is the one re-sent when it slides | A same-named cookie tossed in from a sibling subdomain, sent first, signed the owner out (confirmed live) |
| Development token (final) | `collector`, `up` and `host run` log one warning at start when `HENNERY_DEV_TOKEN` is set, without its value. `up` no longer hands it to its collector child. | A service definition still setting it would look as if it protected something |
| Stream expiry (final) | `session_ended` sleeps at most `SESSION_RECHECK` (an hour) before re-checking against the wall clock. No test: it needs a clock seam that does not exist yet. | Tokio's monotonic clock does not count a suspend, so a stream could outlive its session after a laptop sleep |

Task 3's revert-probe 3 as written fails for the wrong reason: with the rename left in, `rename` of a temp file that was never created errors, so the tests fail at `announce_setup(..).unwrap()` before the symlink assertion. The variant used replaces the rename with `Ok(())`, and then only `a_symlink_at_the_setup_link_is_replaced_and_its_target_left_alone` fails.

Tests: 439 in the workspace, up from 393 before this plan (the plan's 431, plus the tests added in review fix rounds and the final wave).

Still open: the spec amendments listed in this plan, and the hand-offs in "After this plan".

**Spec write-back (2026-10-01):** this plan's spec amendments and spec-level deviations are applied: in #21 (kernel, umbrella) and #22 (ACP core, frontend, distribution). Notes above that call them still to be applied are history.

**Debt sweep (2026-10-02):** closed from "Found in execution and the final review":
- axum's plain-text 400/415/422 rejections are now fixed `ApiError`s, on the auth routes and enrollment (#19) and on the session API (#25);
- `host join`'s stdin read is bounded (#32);
- `rfc3339` has one copy, in the kernel (#32);
- `setup-url`'s hard-link test (#35).

Also: every HTML response, the placeholder `GET /` included, carries kernel §7.2's CSP from one layer over the router (#30).

## Scope

Plan 3b, as 3a's "After this plan" lists it, does not fit in one plan of right-sized tasks. It has two halves that a reviewer can accept or reject separately:
- the operator's credential path: setup, login, sessions, the browser rules, step-up, and removing the bearer;
- the collector's configuration surface: several listeners, `config.toml` with its precedence rules, and the admin socket.

The `owner_id` backfill is a third, mechanical piece. It touches about 80 statements in the sessions store (decision 1). The plan is **split**:
- **3b-i, this plan (7 tasks):** setup, the owner's password, `public_url`, login and logout, auth sessions and the cookie, the browser rules on every operator route, step-up with its endpoint, and the session list and revoke. The development bearer goes. Two of 3a's deferred items are folded in: M3 (loopback gets its own limiter entry) and M5 (the host-side half: the pairing code on stdin).
- **3b-ii, next (outlined in "After this plan"):** several listeners, `config.toml`, the admin socket, `owner_id` on the older tables with every query filtering by it, and `/healthz` and `/readyz`.
- **3c, after it:** passkeys (unchanged from 3a's outline).

**In:**
- Kernel:
  - the migration for `owners`, `password_credentials`, `auth_sessions` and `settings`;
  - `Operator` (setup token and file, owner, password, `public_url`, sessions, limiter);
  - `PublicUrl`;
  - the browser rules, `operator_only`, `require_step_up`;
  - the setup, login, logout, step-up and session handlers;
  - `Policy::LOGIN`, the separate step-up limiter, and the limiter's loopback entry;
  - the static setup page, the fragment-borne token and the 16 KiB body limit;
  - `session_ended`, so a session's streams end with it;
- Wire: the five REST types.
- Collector: `AppState.operator`; every operator route behind the cookie; step-up on minting and host revoke.
- Binary: the setup announcement and placeholder, no `--dev-token`, the join code on stdin.
- Testkit: `operator_client`, `PUBLIC_URL`, `OWNER_PASSWORD`; every harness on the cookie.

**Out** (later plans; see "After this plan"):
- listeners, `config.toml`, the admin socket, the `owner_id` backfill, health checks (3b-ii);
- passkeys and step-up by passkey (3c);
- the setup page and the login form themselves (the frontend plan);
- `PATCH /api/settings` and changing `public_url` (with its own step-up);
- the step-up actions whose endpoints do not exist yet (gateway, stdio servers, session delete, hat purge);
- a password change or reset (the admin socket's reset is 3b-ii);
- identity from a fronting proxy (deferred by the spec).

**Where the earlier hand-offs land:**

| Hand-off (3a "After this plan") | Here |
|---|---|
| Setup link, owner password, `public_url` | Tasks 1 and 3 (decisions 2 to 5). The default hat's name is left for the hats plan (decision 3) |
| `owners` and `owner_id` everywhere | `owners` and `owner_id` on the new tables (Task 1). The backfill of the older tables goes to 3b-ii (decision 1) |
| Login and logout, rate limit, constant-time failure path | Task 4 (decision 6) |
| `auth_sessions`, the cookie, `GET/DELETE /api/auth/sessions` | Tasks 2 and 6 (decision 7) |
| Browser routes: `Origin`, `Sec-Fetch-Site`, JSON only, the exemptions, tests on every listener | Tasks 4 and 5 (decisions 8 and 13). The route table runs on the one listener; "every listener" comes with 3b-ii's listeners |
| Step-up on minting and revoking hosts | Task 6 (decision 10), and on session revoke |
| Several listeners, `config.toml`, the admin socket | 3b-ii |
| The development bearer off every production route | Task 5 (decision 11) |
| M3: the overflow bucket catches loopback too | Task 4 (decision 9) |
| M5: secrets in argv | `--dev-token` is gone (Task 5). The code of `host join` can come on stdin (Task 7, decision 12) |
| "A global budget across every tracked address … belongs with 3b's login limit" | Not added (decision 6): nothing yet shows it is needed |

## Decisions this plan makes where the spec is silent

These were confirmed by a stronger-model security review on the maintainer's behalf (2026-10-02, "ready after amendments"). Each gives the choice, the alternatives, and the cost if it is wrong. Decisions the review changed are marked "(amended after the security review of 2026-10-02)". Items marked **(amendment)** depart from explicit spec text and should be written back into it.

**What the review changed:**
- A1: step-up gets a limiter of its own (decisions 6 and 10).
- A2: the proxy lock-out note in "After this plan".
- A3: ending a session ends its streams (decision 7).
- A4: the link's expiry, the proxy path and the setup headers (decisions 2 and 3).
- A5: the operator's own queries join the 3b-ii `owner_id` list.
- A6: recovery (decision 4).
- A7: no `GET` changes state (decision 8).
- A8: the static pages (decision 13).
- A9: decision 14 shipped in PR #9.
- A10: redacting `Debug` is tested (decision 5).
- A11: rebuilt and replayed.
- Optional hardening taken:
  - the token in the link's fragment (decision 16);
  - a 16 KiB body limit (decision 17);
  - the refused origin logged at debug level (decision 8).

  The skipped and recorded items are in "After this plan".

1. **The split, and where `owner_id` goes (amendment).**
   - **Choice:** this plan's new tables carry `owner_id` from their first migration. The older tables are backfilled in 3b-ii: `hosts`, `pairing_codes`, and the sessions store's `sessions`, `turns`, `events`, `session_catalog`, `pending` and `answer_queue`. That is also when every query starts filtering by it.
   - **Why:** v1 has exactly one owner, so the filter isolates nothing yet (umbrella §15: teams are "later"). It touches about 80 statements in `store.rs`, which would roughly double this plan's churn without making the credential path any safer.
   - **Alternatives:** do it here (one very large Task 5), or leave it with no plan (the spec says every table).
   - **Cost if wrong:** until 3b-ii, a row written without an owner has to be backfilled. With a single owner that is one `UPDATE … SET owner_id = <the owner>` per table in that migration.
   - (The brief for this plan cites "decision 2 (`owner_id` everywhere)". In `docs/README.md`, decision 2 is about per-hat agent config. The `owner_id` rule is 3a's decision 2, kernel §1 and umbrella §7.4.)
2. **The setup link** (amended after the security review of 2026-10-02).
   - The token is 32 random bytes as hex, with only its SHA-256 kept, and **only in memory**: a restart issues a new one, and the old one is dead by construction (kernel §3.1). The alternative, a database row, needs explicit invalidation at start and outlives a crash.
   - The link is `http://localhost:<bound port>/setup#<token>`, with the token in the fragment (decision 16). There is no `public_url` before setup; `config.toml` may provide one in 3b-ii.
   - It is valid for an hour (`SETUP_TOKEN_TTL_SECS`). An expired link answers 401 `invalid_setup_token`, and the file stays until the next start replaces it with a fresh link.
   - It is written to `<data>/setup-url` **after the listener is bound**: the link names the port, and tests read the file's existence as "serving". It is written as a 0600 file created with `O_EXCL` under a random temporary name, then renamed into place. A symlink or hard link planted at `setup-url` is replaced, never written through. Alternative: `open(O_NOFOLLOW)` on the final name, which a hard link defeats.
   - It is printed only when stdout is a terminal. Otherwise only the file's path is logged, and a test pins that the token appears in neither output stream.
   - The file is removed when setup succeeds, and at start once there is an owner.
   - `/setup` serves a static page, and `/setup.js` its script (decision 16), until the frontend exists. The script reads the token from `location.hash`, removes it from the address bar, and POSTs it with the password and `public_url` (pre-filled with `location.origin`).
   - Every `/setup`, `/setup.js` and `/api/setup` response carries `Referrer-Policy: no-referrer` and `Cache-Control: no-store`. The page carries kernel §7.2's `Content-Security-Policy` (`script-src 'self'`, with no inline script).
   - **Cost if wrong:** without the page, the printed link opens a 404. Setup still works with `curl`.
3. **Setup's own `Origin` rule, and setup signs the owner in** (amended after the security review of 2026-10-02).
   - Before setup there is no `public_url` to check against. So `POST /api/setup` requires an `Origin` equal to the origin of the `public_url` it submits; a missing one is refused, 403 `origin_mismatch`. The token is the authentication. The `Origin` check makes sure the stored origin is the one this browser uses: a mismatch would lock the operator out of every state-changing route afterwards.
   - It is **not** a CSRF defence. Without the 256-bit token a cross-site page can do nothing, whatever `Origin` it sends.
   - Behind a reverse proxy, the token passes through the proxy, in the `POST /api/setup` body. With the token in the link's fragment (decision 16) it is never in a request line, so it does not land in the proxy's access log.
   - The checks run in this order:
     1. the URL's shape (400 `invalid`);
     2. `Origin` (403);
     3. an owner already exists (409 `already_set_up`);
     4. the token (401 `invalid_setup_token`);
     5. the password (400 `invalid`).

     The token is used up only when the owner's row commits, so no failed check costs a restart.
   - A success answers 201 `{public_url}` with the session cookie. The operator who just typed the password is the owner, and the session counts as stepped up.
   - The default hat's name is **not** asked for yet. The hats plan adds it to `SetupRequest` as an additive change, together with the default hat it names.
   - **Cost if wrong:** the operator needs one extra login after setup, which is trivial.
4. **`public_url` is kept as the browser's `Origin` serialisation** (amended after the security review of 2026-10-02).
   - It is parsed with `url` and stored as `Url::origin().ascii_serialization()`: lowercase scheme and host, no default port, no trailing slash, IPv6 in brackets, IDNA punycode. So `https://Hennery.Example:443/` matches the `Origin: https://hennery.example` a browser sends, and the two compare as plain strings.
   - It must be `https://`, or `http://` to `localhost`, `127.0.0.0/8` or `[::1]` (umbrella §7.5). There may be no path, query, fragment or credentials.
   - The cookie's `Secure` flag comes from `public_url`'s scheme, **never from the peer address**. Behind a TLS proxy on loopback every peer is `127.0.0.1`, yet the browser is on `https://`.
   - **Cost if wrong:** a stricter comparison (the raw string) locks out an operator who typed the URL another way. A looser one (host only) lets another port or scheme of the same host pass the `Origin` check.
   - **A known lock-out, with no recovery in this plan:**
     - The trigger: the operator moves the collector after setup. For example, `public_url` is `http://localhost:7117` and `up` then runs on another port, or behind a proxy on another origin.
     - The effect: every state-changing request, login included, is refused 403 `origin_mismatch`. Only `GET`s still work.
     - Why nothing here fixes it: 3b-i has no `PATCH /api/settings`, no admin socket and no `config.toml`.
     - Today's recovery: stop the collector and edit the row by hand, with `UPDATE settings SET value = '<new origin>' WHERE key = 'public_url'` in `hennery.db`.
     - 3b-ii owes a proper reset path (see "After this plan"). Any reset path must also replace `Operator`'s cached `public_url`, not only the row.
   - `load_public_url` re-parses the stored value when the collector opens its database, so a row edited by hand into something invalid stops the start with a clear error instead of serving with it.
   - **A forgotten password, recovered by hand** (until 3b-ii's admin-socket reset): stop the collector, then run `DELETE FROM auth_sessions; DELETE FROM password_credentials; DELETE FROM settings; DELETE FROM owners;` against `hennery.db`, in that order (the foreign keys point at `owners`). Start it again: with no owner, it writes a fresh setup link.
5. **Passwords** (amended after the security review of 2026-10-02).
   - Argon2id via `password-auth` 1.0.0 with its defaults (19 MiB, t=2, p=1, PHC string).
   - At least 12 characters; at most 1024 bytes, which bounds each hash.
   - At most **two** hashes or verifications run at once (a `tokio` semaphore); others wait their turn. Each costs about 19 MiB, and per-address limits do not bound a flood from many addresses.
   - Before setup, a check runs against a dummy hash computed once, so every check costs one Argon2 verify.
   - `argon2` and `blake2` are built at `opt-level = 3` in the dev profile. Unoptimised, every test that logs in would take seconds, worse with four copies at once. Release builds are optimised anyway.
   - `SetupRequest`, `LoginRequest` and `StepUpRequest` implement `Debug` by hand and leave the password out. A `format!("{req:?}")` test pins each in the task that adds it (3, 4 and 6).
   - **Cost if wrong:** with two permits, a login flood queues the owner's own login behind it. More permits trade memory for latency.
6. **Login** (amended after the security review of 2026-10-02).
   - The body is `{password}` only: v1 has one owner (umbrella §7.3). A success answers 204 with the cookie. Setup and login both record a step-up.
   - Rate limiting reuses 3a's `Limiter` with `Policy::LOGIN`: 5 wrong passwords per minute per address, then 60 s, doubling to at most an hour.
   - Step-up has its own `Limiter` with the same policy (decision 10), so its budget is separate. Login is the path an anonymous guesser reaches, and behind a proxy it is the one a flood locks out. A step-up needs a live session already, and must stay usable while login is locked.
   - Every attempt counts from the moment it starts; only a success clears the address.
   - **The constant-time failure path is structural:** an attempt that is not rate limited runs exactly one Argon2 verify, right password or wrong. A rate-limited one runs none, so even the right password gets 429 while its address is locked out. `password-auth` compares hashes in constant time. A test pins the count (`Operator::verifications`); wall-clock timing is not asserted, because it would flake under load.
   - Before setup, login is refused by the browser rules (403 `setup_required`) before any check.
   - **Accepted cost:** behind a reverse proxy every client is the proxy's address, often loopback. A flood of wrong passwords there locks the owner out too, for up to an hour; a collector restart clears it, because the limits live in memory. `X-Forwarded-For` is forgeable and is not trusted (as in 3a). 3a's handed-on "global budget" is not added.
7. **Sessions** (amended after the security review of 2026-10-02).
   - The token is 32 random bytes as hex. The row's id is its SHA-256 hex (kernel §3.2).
   - It expires 30 days after its last use. A use slides the expiry **at most once every 60 s**, so a busy client does not write on every request. When a request slides it, the response sends the cookie again: otherwise the browser's `Max-Age` would run out 30 days after login even while the session is in use.
   - The cookie is `hennery_session=<token>; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000`, plus `; Secure` unless `public_url` is loopback `http://`.
   - Expired rows are deleted whenever a session is opened. The `User-Agent` is kept without control characters, capped at 256 characters.
   - `GET /api/auth/sessions` lists the live sessions, most recently used first, with `current` marked. The listed `id` is the stored hash: it names a session without revealing its cookie.
   - `DELETE /api/auth/sessions/{id}` (step-up) answers 204 or 404. Revoking the request's own session is allowed and clears its cookie.
   - `POST /api/auth/logout` sits behind the browser rules but needs no session. It answers 204 and clears the cookie, ending the session if there is one.
   - **Ending a session ends its streams.**
     - The mechanism: `Operator` keeps a `tokio::sync::watch` generation, bumped by every `revoke_session`, so by a revoke and by a logout (and by 3b-ii's password reset, which must end every session).
     - The stream: `GET /api/stream/sessions/{id}` takes the request's `Authenticated` session and ends (`auth::session_ended`) when a bump finds that session gone. It also ends at its expiry, re-read at that moment, since the session's other requests may have slid it.
     - The check at subscription: it runs once when it subscribes, so an ending between the cookie check and the subscription is not missed.
     - Why: without it, a revoked device could keep reading a session's events for as long as its SSE connection lasted.
   - **Cost if wrong:** a 60 s slide is invisible to anyone; the list's `last_seen_at` is only as fine as that.
8. **The browser rules** (kernel §3.3, `origin::browser_rules`) (amended after the security review of 2026-10-02).
   - **Which methods count as state-changing:** every method except `GET` and `HEAD`, not only the four the spec names. `OPTIONS`, a CORS preflight, is refused like a `POST`: hennery serves no CORS.
   - **State-changing requests:**
     - `Origin` must equal `public_url`'s origin, and a missing or non-ASCII one is refused (403 `origin_mismatch`).
     - Before setup they are all refused (403 `setup_required`).
     - The body must be JSON: a `Content-Type` other than `application/json` (parameters allowed) is refused, 415 `unsupported_media_type`, and so is a body with no `Content-Type`. A body-less request needs no `Content-Type` (no `Transfer-Encoding`, and `Content-Length` absent or 0): park, close, logout, revoke.
   - **`GET` and `HEAD`:**
     - `Sec-Fetch-Site` may be `same-origin` or `none`. Any other value is refused, 403 `cross_site`.
     - **A missing `Sec-Fetch-Site` is accepted**: `curl` and older browsers send none, and the `SameSite=Strict` cookie already keeps cross-site `GET`s unauthenticated.
     - **No `GET` or `HEAD` route changes state; one that must is a `POST`.** Accepting a missing `Sec-Fetch-Site` relies on this. `origin.rs`'s module doc says so too, for whoever adds the next route.
     - A present `Origin` must match.
   - **Order:** the browser rules wrap the session check, so a cross-origin request gets 403 before its missing cookie gets 401.
   - An `origin_mismatch` logs the expected and the received origin at debug level (the received one `Debug`-escaped, since a client chose it), so an operator behind a misconfigured proxy can see why.
   - **Cost if wrong:** refusing a missing `Sec-Fetch-Site` would break `curl` and pre-16.4 Safari, and would add nothing that `SameSite=Strict` does not already give.
9. **Loopback in the limiter** (3a's M3).
   - Every `127.0.0.0/8` address is keyed as `127.0.0.1`. `::1` stays itself: masking it to its /64 would give `::`, which is not loopback.
   - Loopback always gets its own entry, past capacity too, so a flood from elsewhere cannot push `hennery up`'s own enrollment, or a local proxy's logins, into the shared overflow budget. Loopback is not exempt: its budget is anyone's.
   - The alternative, per-address loopback entries, would give a local process 16 million separate budgets on Linux, where the whole /8 reaches `lo`.
   - **Cost:** at most two entries past capacity.
10. **Step-up** (kernel §3.4) (amended after the security review of 2026-10-02).
    - `last_step_up_at` is fresh for **strictly less than** 300 s.
    - Setup and login stamp it. `POST /api/auth/step-up/password {password}` stamps it again (204). A wrong password answers 401 `invalid_password`.
    - Rate limiting uses **its own limiter**, `Operator::step_up_limiter` (`Policy::LOGIN`), with one Argon2 verify per attempt that gets in, as at login:
      - five wrong step-ups from an address lock out step-up from it, and leave login alone;
      - a login flood from the owner's address (a shared proxy, say) does not stop a signed-in owner stepping up.
    - The alternative, one budget for both, let an anonymous flood at the login route deny step-up to the owner who is already signed in.
    - `require_step_up` (403 `step_up_required`) guards `POST /api/hosts/pairing-codes`, `DELETE /api/hosts/{id}` and `DELETE /api/auth/sessions/{id}`. The other actions in §3.4 get it when their endpoints exist.
    - It is a route layer inside `operator_only`, so it reads the session that the cookie check put in the request.
    - **Cost if wrong:** a stale session gets one extra password prompt.
11. **The development bearer goes entirely.**
    - `DevToken`, `require_bearer`, `MIN_DEV_TOKEN_LEN`, `--dev-token` and `HENNERY_DEV_TOKEN` are gone; clap refuses `--dev-token` as an unknown argument, so an old service definition fails loudly.
    - `AppState::new(store, hosts, operator)`.
    - **No test-only production path:**
      - The testkit harnesses open sessions through `Operator` directly, the way login does (`hennery_testkit::operator_client`).
      - The CLI tests set the collector up through its `setup-url` file, the way an operator does.
    - `HOST_SECRET_VARS` keeps stripping `HENNERY_DEV_TOKEN` from every agent: an operator's shell may still export it from before.
    - **Tests removed:**
      - `a_short_dev_token_is_refused_at_start` becomes `the_development_token_flag_is_gone`;
      - `a_short_dev_token_stops_the_collector_before_it_touches_the_data_dir` goes, with no token left to check;
      - `auth.rs`'s `a_request_without_or_with_the_wrong_bearer_token_is_rejected` becomes the route table;
      - the two `DevToken` unit tests go.
    - **Tests kept, re-aimed:** `ups_host_child_does_not_inherit_the_operator_token` and `ups_agents_never_see_the_operator_token_or_the_pairing_pipe`. Both still pin `HENNERY_DEV_TOKEN` stripping, and the second still pins the pairing pipe.
12. **`host join` takes the code on stdin** (the host-side half of 3a's M5).
    - `code` becomes optional. Left out, one line is read from stdin, with a `Pairing code:` prompt on stderr when stdin is a terminal. An empty line is refused.
    - The positional code is kept for scripts. Kernel §2's rule ("never as CLI flags") is kept in spirit: it is single-use and dies in ten minutes, as 3a's decision 5 argued.
13. **Exempt routes** (kernel §3.3) (amended after the security review of 2026-10-02):
    - `POST /api/hosts/enroll` (the code) and `GET /api/hosts/ws` (the `hello` proof), merged outside `operator_only`;
    - `POST /api/setup` (the token, with its own `Origin` rule, decision 3);
    - `GET /` (the placeholder) and `GET /setup`, `GET /setup.js` (decision 16): static pages with no data, outside the browser rules and the cookie.

    `/healthz` and `/readyz` do not exist yet (3b-ii). A route table test pins every operator route, and a test pins that enrollment and the WebSocket need neither a cookie nor an `Origin`.
14. **Every transaction is `IMMEDIATE` (amendment to 3a's decision 2)** (amended after the security review of 2026-10-02): **shipped in PR #9** (`66ec866`, merged as `a005dfa`) ahead of this plan, so this plan no longer carries it.
    - Found while building this plan. With a third writer on `hennery.db` (the operator, for setup and for session slides), enrollment failed at once with `database is locked` in about one parallel CLI run in three. The all-in-one host then exited, and the tests timed out.
    - **The cause:** a deferred transaction that reads first (`Hosts::enroll`, and most of the store's) fails on its first write with `SQLITE_BUSY_SNAPSHOT` whenever another connection committed after its read. The busy timeout is **not** applied to that case, so 3a's "the busy timeout covers the overlap" was wrong.
    - **The fix, in PR #9:** `db::configure` sets `TransactionBehavior::Immediate` on every connection, so `transaction()` takes the write lock when it begins and waits out the busy timeout. It serialises the writers until the single writer thread (kernel §1). Its test is `a_transaction_holds_the_write_lock_from_its_start`.
    - **Cost:** transactions that only read also take the write lock. There are none in the tree today.
15. **`Operator` on its own connection.** It opens `hennery.db` like `Hosts` does, and runs the same kernel migration list (`schema.rs`, which moves out of `hosts.rs`). This is three connections, serialised by decision 14. `public_url` is cached in memory, loaded at open and replaced by setup, because every browser request reads it.
16. **The setup token travels in the link's fragment (amendment to kernel §3.1)** (added after the security review of 2026-10-02).
    - The link is `…/setup#<token>`, not kernel §3.1's `…/setup/<token>`, and the `setup-url` file holds that form.
    - A browser never sends a fragment, so the token never appears in a request line, and so in no server log, proxy access log or `Referer`. It leaves the browser only in the `POST /api/setup` body.
    - The page's script is the file `/setup.js`, not inline, so the page's CSP needs no hash (kernel §7.2).
    - **Alternatives:**
      - the path form, which leaks into access logs and `Referer`;
      - a query string, which leaks the same way.
    - **Cost if wrong:** a client that cannot run the script, `curl` say, has to take the token out of the link by hand. The body format is the same either way.
17. **A 16 KiB body limit on the auth routes** (added after the security review of 2026-10-02).
    - `DefaultBodyLimit::max(MAX_BODY_BYTES)` (16 KiB) covers `/api/setup` and `/api/auth/*`, where axum's default is 2 MiB. A password is at most 1024 bytes.
    - A larger body is refused 413 before it is parsed, and before any password check.
    - **Cost if wrong:** none: no legitimate auth body comes near it.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- After every task these pass:
  - `cargo fmt --all --check` (`max_width = 120`);
  - `cargo clippy --workspace --all-targets --locked -- -D warnings`;
  - `cargo clippy -p hennery --locked -- -D warnings` (test hooks off);
  - `cargo test --workspace --locked`;
  - `cargo run -p hennery-proto --bin gen -- --check` and `cargo run --bin gen -- --check`.
- A task that changes a `Cargo.toml` runs one `cargo build --workspace` **without** `--locked` first, and commits the updated `Cargo.lock`. New crates are pinned with `=x.y.z` in `[workspace.dependencies]` and taken from there with `.workspace = true`.
- Generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated with `cargo run -p hennery-proto --bin gen` whenever a wire type changes. In `codegen.rs`, new root types go in both `add!` lists.
- Setup (kernel §3.1): "generate a 256-bit setup token and write `<public_url or http://localhost:PORT>/setup/<token>` to `<data>/setup-url` (mode 0600). The full link is printed to the terminal **only when stdout is a TTY** … Valid for 1 hour, single use; a restart before setup issues a new one and invalidates the old." Decision 16 amends the link's form to `/setup#<token>`.
- `public_url` "must be `https://` or a loopback `http://` origin" (kernel §3.1).
- Login (kernel §3.2): "Argon2id (PHC string, `password-auth`), verification on a blocking thread. Login attempts are rate limited per client address (5 per minute, then exponential backoff), with a constant-time failure path."
- Sessions (kernel §3.2): "server-side (`auth_sessions`: random 256-bit id hashed at rest). Cookie `hennery_session`: `HttpOnly`, `Secure` (except loopback), `SameSite=Strict`, `Path=/`. 30-day sliding expiry."
- Browser rules (kernel §3.3):
  - "State-changing endpoints accept only `application/json`."
  - State-changing browser routes: "Must match `public_url`; a missing `Origin` is rejected."
  - `GET`: "`same-origin` or `none` accepted, anything else rejected; a present `Origin` must match."
- Step-up (kernel §3.4): "a password or passkey check within the last **5 minutes** … Without a fresh check the endpoint answers 403 `step_up_required`."
- "Secrets are never accepted as CLI flags (they would show in process listings)" (kernel §2).
- "Every API call is authenticated — there is no 'open on the LAN' mode" (umbrella §7.4).
- No global installs: tooling comes from the flake dev shell.
- Commits follow Conventional Commits (`feat(kernel): …`) and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **A `public_url` typed differently from the origin the browser sends:** `https://Hennery.Example:443/` against `Origin: https://hennery.example`.
   - Expected: setup accepts it, stores the browser's form, and every later state-changing request from that browser passes.
   - Tests: Task 1 `public_urls_are_normalised_to_their_browser_origin`; Task 3 `setup_creates_the_owner_signs_them_in_and_happens_once`.
2. **Several connections writing `hennery.db` at once:** a setup, or a session slide, landing during an enrollment or an ingest.
   - Expected: each waits its turn. None fails with `database is locked`, and the all-in-one host still pairs.
   - Tests: `a_transaction_holds_the_write_lock_from_its_start` (PR #9, decision 14). Every CLI test that runs `up` then signs in while the host pairs, e.g. `up_pairs_its_own_host_once`, under four parallel copies.
3. **An operator route added after the auth layer, or a cross-site page aiming at one.**
   - Expected: every operator route in the route table answers 401 without a live cookie. State-changing routes answer 403 without the `public_url`'s `Origin`, and `GET`s answer 403 when the browser marks them cross-site. Enrollment and the host WebSocket need neither.
   - Tests: Task 5 `every_operator_route_needs_the_session_cookie`, `every_operator_route_applies_the_browser_rules`, `enrollment_and_the_host_socket_need_neither_a_session_nor_an_origin`; revert-probed by moving a route after the layer. They cover the routes the table lists: a new route that is not added to the table is not caught (see "After this plan").
4. **The setup link leaking, or being redirected:** a service's log collector, a proxy's access log, a `Referer`, or a symlink planted at `setup-url`.
   - Expected: the token appears in neither output stream and never in a request line (it is the fragment). The file is 0600, a planted symlink's target is untouched, and the file is gone after setup. Setup responses are never cached and send no `Referer`.
   - Tests: Task 3 `an_unset_collector_writes_its_setup_link_to_a_private_file_and_never_to_its_output`, `a_symlink_at_the_setup_link_is_replaced_and_its_target_left_alone`, `the_setup_link_is_written_privately_and_removed_by_setup`, `setup_responses_are_never_cached_nor_referred`, `the_setup_page_is_static_and_reads_the_token_from_the_fragment`.
5. **A password guesser at login or at step-up, and the owner caught in the middle.**
   - Expected at login: four wrong passwords are free and the fifth locks the address out. From then on even the right password gets 429 with a `Retry-After`, and no locked-out attempt runs a password check. An oversized body is refused 413 before anything is checked.
   - Expected at step-up: the same, on a budget of its own. A step-up lockout leaves login alone, and a login lockout (a flood through a shared proxy, say) does not stop a signed-in owner stepping up.
   - Tests: Task 4 `wrong_passwords_lock_the_address_out_and_each_attempt_checks_once`, `a_right_password_clears_the_count`, `loopback_keeps_one_entry_of_its_own_past_capacity`, `an_oversized_body_is_refused_before_it_is_parsed`; Task 6 `wrong_step_ups_lock_out_step_up_only`, `a_locked_out_login_does_not_block_step_up`.
6. **A session in use for more than 30 days, one idle for five minutes, and one revoked while it streams.**
   - Expected: use keeps both the server row and the browser's cookie alive (the cookie is re-sent when the expiry slides). A session last checked five minutes ago must step up before minting, revoking a host or revoking a session. A revoked or signed-out session's open event stream ends within a second; another session's stays open.
   - Tests: Task 2 `a_session_authenticates_until_it_expires_and_use_slides_its_expiry`; Task 5 `a_request_that_slides_the_session_sends_its_cookie_again`; Task 6 `minting_revoking_a_host_and_revoking_a_session_need_a_fresh_password_check`, `ending_a_session_ends_its_open_streams`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `Cargo.toml`, `crates/hennery-kernel/Cargo.toml`, `crates/hennery-testkit/Cargo.toml`, `Cargo.lock` | `password-auth`, `url`; `tokio` and `time` for the kernel; the kernel and `reqwest` for the testkit's library; `opt-level = 3` for `argon2` and `blake2` | 1, 5, 6 |
| `crates/hennery-kernel/src/schema.rs` | The kernel's migrations (moved from `hosts.rs`), with the operator tables | 1 |
| `crates/hennery-kernel/src/operator.rs` | `Operator`, `PublicUrl`, `SetupOutcome`, `SetupLink`, `Authenticated`, `AuthSession`, the cookie helpers | 1–4 |
| `crates/hennery-kernel/src/auth_api.rs` | `/api/setup`, `/api/auth/login`, `logout`, `step-up/password`, `sessions`; the body limit | 3, 4, 6 |
| `crates/hennery-kernel/src/setup_page.rs` | `/setup` and `/setup.js`, and the no-referrer, no-store headers | 3 |
| `crates/hennery-kernel/src/origin.rs` | `browser_rules` | 4 |
| `crates/hennery-kernel/src/auth.rs` | `operator_only`, `require_operator`, `require_step_up`, `session_ended` (replaces `DevToken`) | 5, 6 |
| `crates/hennery-kernel/src/ratelimit.rs` | `Policy::LOGIN`; loopback's key and entry | 4 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs` | `SetupRequest`, `SetupResponse`, `LoginRequest`, `StepUpRequest`, `AuthSessionItem` | 3, 4, 6 |
| `crates/hennery-sessions/src/lib.rs`, `api.rs`, `hosts.rs` | `AppState.operator`; the operator routes behind the cookie; step-up on minting and revoke; the stream ends with its session | 3, 5, 6 |
| `crates/hennery/src/main.rs` | The setup announcement; no `--dev-token`; the join code on stdin | 3, 5, 7 |
| `crates/hennery-testkit/src/lib.rs` | `operator_client`, `PUBLIC_URL`, `OWNER_PASSWORD` | 5 |
| Tests: `crates/hennery-kernel/tests/{operator,auth_sessions}.rs`, `crates/hennery-testkit/tests/{setup,login,step_up,auth,e2e,reconcile,join,pairing,ws_ingest_error}.rs`, `crates/hennery/tests/cli.rs` | | all |

All commands run from the repository root inside the dev shell (`nix develop`, or direnv). Work on a feature branch off `main` (e.g. `feat/operator-auth`). Each task leaves the workspace compiling, clippy-clean and green, and the binary working: `up` keeps connecting its host through every task. Tasks 1 to 4 add the new routes while the bearer still guards everything else; Task 5 switches every route over in one step.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "Replace the whole of `path` with:" overwrites the file with the block (and a final newline).
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate; they change no source file. The plan was replayed exactly this way, from its own text, onto `a005dfa`.

---

### Task 1: The operator, their password and the one-time setup token

**Files:**
- Modify: `Cargo.toml` (`password-auth`, `url`, the `argon2`/`blake2` profile), `crates/hennery-kernel/Cargo.toml`, `Cargo.lock`
- Create: `crates/hennery-kernel/src/schema.rs`, `crates/hennery-kernel/src/operator.rs`
- Modify: `crates/hennery-kernel/src/lib.rs`, `crates/hennery-kernel/src/hosts.rs` (its migrations move to `schema.rs`)
- Test: `crates/hennery-kernel/tests/operator.rs`

**Interfaces:**
- Produces (`hennery_kernel::operator`):
  - consts `SETUP_TOKEN_TTL_SECS = 3600`, `MIN_PASSWORD_CHARS = 12`, `MAX_PASSWORD_BYTES = 1024`, `MAX_CONCURRENT_HASHES = 2`;
  - `struct PublicUrl`, with `parse(&str) -> Result<PublicUrl, String>`, `origin() -> &str` and `is_https() -> bool`;
  - `enum SetupOutcome { Done { owner_id: String }, AlreadySetUp, InvalidToken, Invalid(String) }`;
  - `struct Operator`, with:
    - `open(&Path)` and `open_in_memory()`, both `-> anyhow::Result<Operator>`;
    - `owner_id() -> Result<Option<String>>`, `is_set_up() -> Result<bool>`, `public_url() -> Option<PublicUrl>`;
    - `issue_setup_token(now: i64) -> Result<Option<String>>`: `None` once set up;
    - `set_up(token, password, public_url: &str, now: i64) -> Result<SetupOutcome>`: blocking, it hashes;
    - `verify_password(&str) -> Result<bool>`: blocking;
    - `async check_password(self: &Arc<Self>, String) -> Result<bool>`: a blocking thread, at most two at once;
    - `verifications() -> u64`.
- Produces (`hennery_kernel::schema`, crate-private): `COMPONENT`, `MIGRATIONS`. The second migration creates `owners`, `password_credentials`, `auth_sessions` and `settings`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-kernel/tests/operator.rs`:

```rust
//! The operator and their setup (kernel spec §3.1, §3.2, §11): the
//! one-time setup token, the owner's password and the `public_url`.

use hennery_kernel::operator::{MAX_PASSWORD_BYTES, Operator, PublicUrl, SETUP_TOKEN_TTL_SECS, SetupOutcome};
use std::sync::Arc;

const NOW: i64 = 1_800_000_000;
const PASSWORD: &str = "correct horse battery";

#[test]
fn a_setup_token_is_single_use_and_creates_one_owner() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    assert_eq!(
        op.set_up("0".repeat(64).as_str(), PASSWORD, "https://hennery.example", NOW)
            .unwrap(),
        SetupOutcome::InvalidToken
    );
    let SetupOutcome::Done { owner_id } = op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap() else {
        panic!("setup failed");
    };
    assert_eq!(op.owner_id().unwrap(), Some(owner_id));
    assert_eq!(
        op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::AlreadySetUp
    );
    assert_eq!(op.issue_setup_token(NOW).unwrap(), None);
    assert_eq!(op.public_url().unwrap().origin(), "https://hennery.example");
}

#[test]
fn a_setup_token_expires_after_an_hour() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    let later = NOW + SETUP_TOKEN_TTL_SECS;
    assert_eq!(
        op.set_up(&token, PASSWORD, "https://hennery.example", later).unwrap(),
        SetupOutcome::InvalidToken
    );
    assert_eq!(op.owner_id().unwrap(), None);
}

/// Kernel spec §3.1: a restart before setup issues a new token and
/// invalidates the old.
#[test]
fn a_restart_before_setup_invalidates_the_old_token() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let old = Operator::open(&db).unwrap().issue_setup_token(NOW).unwrap().unwrap();
    let op = Operator::open(&db).unwrap();
    assert_eq!(
        op.set_up(&old, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::InvalidToken
    );
    let new = op.issue_setup_token(NOW).unwrap().unwrap();
    assert_ne!(old, new);
    assert!(matches!(
        op.set_up(&new, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::Done { .. }
    ));
    // The owner and the public URL survive a restart.
    let reopened = Operator::open(&db).unwrap();
    assert!(reopened.is_set_up().unwrap());
    assert_eq!(reopened.public_url().unwrap().origin(), "https://hennery.example");
}

#[test]
fn a_bad_password_or_public_url_is_refused_and_keeps_the_token() {
    let op = Operator::open_in_memory().unwrap();
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    for (password, url) in [
        ("short", "https://hennery.example"),
        (&*"x".repeat(MAX_PASSWORD_BYTES + 1), "https://hennery.example"),
        (PASSWORD, "http://hennery.example"),
        (PASSWORD, "https://hennery.example/app"),
    ] {
        assert!(
            matches!(op.set_up(&token, password, url, NOW).unwrap(), SetupOutcome::Invalid(_)),
            "{password:?} {url:?}"
        );
    }
    assert!(matches!(
        op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::Done { .. }
    ));
}

#[test]
fn only_the_owners_password_verifies_and_every_check_is_counted() {
    let op = Operator::open_in_memory().unwrap();
    // Before setup: a dummy hash, and never a match.
    assert!(!op.verify_password(PASSWORD).unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    assert!(op.verify_password(PASSWORD).unwrap());
    assert!(!op.verify_password("correct horse battery!").unwrap());
    assert!(!op.verify_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)).unwrap());
    assert_eq!(op.verifications(), 4);
}

/// The public URL is kept as a browser serialises `Origin`, so a
/// `public_url` typed another way still matches (review focus 3).
#[test]
fn public_urls_are_normalised_to_their_browser_origin() {
    for (input, origin, https) in [
        ("https://Hennery.Example", "https://hennery.example", true),
        ("https://hennery.example:443/", "https://hennery.example", true),
        ("https://hennery.example:8443", "https://hennery.example:8443", true),
        ("http://localhost:7117", "http://localhost:7117", false),
        ("http://127.0.0.1:80", "http://127.0.0.1", false),
        ("http://[::1]:7117/", "http://[::1]:7117", false),
        (" HTTPS://hennery.example ", "https://hennery.example", true),
    ] {
        let url = PublicUrl::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
        assert_eq!((url.origin(), url.is_https()), (origin, https), "{input}");
    }
    for bad in [
        "http://hennery.example",
        "http://192.168.1.2:7117",
        "ftp://hennery.example",
        "https://user:pw@hennery.example",
        "https://hennery.example/path",
        "https://hennery.example/?q=1",
        "https://hennery.example/#f",
        "hennery.example",
        "http://localhost.evil.example",
    ] {
        assert!(PublicUrl::parse(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn check_password_verifies_on_a_blocking_thread() {
    let op = Arc::new(Operator::open_in_memory().unwrap());
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, PASSWORD, "https://hennery.example", NOW).unwrap();
    let checks: Vec<_> = (0..4)
        .map(|i| {
            let op = op.clone();
            let password = if i % 2 == 0 {
                PASSWORD.to_string()
            } else {
                "wrong".to_string()
            };
            tokio::spawn(async move { op.check_password(password).await.unwrap() })
        })
        .collect();
    let mut results = Vec::new();
    for check in checks {
        results.push(check.await.unwrap());
    }
    assert_eq!(results, [true, false, true, false]);
    assert_eq!(op.verifications(), 4);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-kernel --test operator`
Expected: FAIL to compile. `error[E0432]: unresolved import hennery_kernel::operator`, and `error[E0433]: cannot find module or crate tokio in this scope` (`tokio` is not a kernel dependency yet).

- [ ] **Step 3: The crates, the schema module and the operator**

`operator.rs` holds the whole store; its tests are the integration tests above. `db.rs` is untouched: decision 14 shipped in PR #9.

In `Cargo.toml`, replace:

```toml
hex = "=0.4.3"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
```

with:

```toml
hex = "=0.4.3"
password-auth = "=1.0.0"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
```

In `Cargo.toml`, replace:

```toml
ts-rs = { version = "12", features = ["serde-json-impl"] }
uuid = { version = "1", features = ["v7", "serde"] }
```

with:

```toml
ts-rs = { version = "12", features = ["serde-json-impl"] }
url = "=2.5.8"
uuid = { version = "1", features = ["v7", "serde"] }
```

Append to `Cargo.toml`:

```toml
# Argon2 is unusably slow unoptimised; every test that logs in pays for it.
[profile.dev.package.argon2]
opt-level = 3

[profile.dev.package.blake2]
opt-level = 3
```

In `crates/hennery-kernel/Cargo.toml`, replace:

```toml
libc = "0.2"
rusqlite.workspace = true
serde_json.workspace = true
sha2.workspace = true
tracing.workspace = true

```

with:

```toml
libc = "0.2"
password-auth.workspace = true
rusqlite.workspace = true
serde_json.workspace = true
sha2.workspace = true
tokio.workspace = true
tracing.workspace = true
url.workspace = true

```

Create `crates/hennery-kernel/src/schema.rs`:

```rust
//! The kernel's tables in `hennery.db` (kernel spec §1.1), migrated as one
//! component beside the sessions store (`db::migrate_component`). Every
//! part of the kernel that opens the database runs the same list, so
//! whichever opens it first creates them all.

/// The kernel's component name in `schema_versions`.
pub(crate) const COMPONENT: &str = "kernel";

pub(crate) const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE hosts (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        public_key TEXT NOT NULL UNIQUE,
        platform TEXT NOT NULL,
        host_version TEXT NOT NULL,
        capabilities TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER);
    CREATE TABLE pairing_codes (
        code_hash TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        used_at INTEGER);
    ",
    // Operator auth (kernel spec §3). The new tables carry `owner_id` from
    // the start; the older ones get it with the backfill (plan 3b-ii).
    "
    CREATE TABLE owners (
        id TEXT PRIMARY KEY,
        contact TEXT,
        created_at INTEGER NOT NULL);
    CREATE TABLE password_credentials (
        owner_id TEXT PRIMARY KEY REFERENCES owners(id),
        phc TEXT NOT NULL,
        updated_at INTEGER NOT NULL);
    CREATE TABLE auth_sessions (
        id_hash TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        user_agent TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL,
        last_step_up_at INTEGER,
        expires_at INTEGER NOT NULL);
    CREATE TABLE settings (
        owner_id TEXT NOT NULL REFERENCES owners(id),
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        PRIMARY KEY (owner_id, key));
    ",
];
```

In `crates/hennery-kernel/src/hosts.rs`, replace:

```rust

use crate::db;
use crate::secret::{random_bytes, sha256_hex};
use anyhow::Result;
```

with:

```rust

use crate::secret::{random_bytes, sha256_hex};
use crate::{db, schema};
use anyhow::Result;
```

In `crates/hennery-kernel/src/hosts.rs`, replace:

```rust
use std::sync::Mutex;

/// The kernel's component name in `schema_versions`.
const COMPONENT: &str = "kernel";

const MIGRATIONS: &[&str] = &["
    CREATE TABLE hosts (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        public_key TEXT NOT NULL UNIQUE,
        platform TEXT NOT NULL,
        host_version TEXT NOT NULL,
        capabilities TEXT NOT NULL DEFAULT '[]',
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER);
    CREATE TABLE pairing_codes (
        code_hash TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        used_at INTEGER);
"];

```

with:

```rust
use std::sync::Mutex;

```

In `crates/hennery-kernel/src/hosts.rs`, replace:

```rust
    fn init(mut conn: Connection) -> Result<Self> {
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self { conn: Mutex::new(conn) })
```

with:

```rust
    fn init(mut conn: Connection) -> Result<Self> {
        db::migrate_component(&mut conn, schema::COMPONENT, schema::MIGRATIONS)?;
        Ok(Self { conn: Mutex::new(conn) })
```

Create `crates/hennery-kernel/src/operator.rs`:

```rust
//! The operator (kernel spec §3): the one owner, their password, the
//! `public_url` setting, and the one-time setup link that creates them.
//!
//! - **One owner.** Setup creates it, once; a second setup is refused.
//! - **The setup token lives in memory only.** A restart before setup
//!   issues a new one, and the old one is dead by construction (kernel
//!   spec §3.1). Only its SHA-256 is kept.
//! - **Passwords are Argon2id PHC strings** (`password-auth`). Hashing and
//!   verifying are CPU- and memory-heavy (about 19 MiB each), so they run
//!   on a blocking thread and at most `MAX_CONCURRENT_HASHES` at once.
//!
//! Every time is seconds since the Unix epoch, passed in by the caller.

use crate::secret::{random_bytes, sha256_hex};
use crate::{db, schema};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// A setup link is valid this long (kernel spec §3.1).
pub const SETUP_TOKEN_TTL_SECS: i64 = 60 * 60;

/// The shortest password setup accepts, in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;

/// The longest password accepted, in bytes: hashing is bounded by it.
pub const MAX_PASSWORD_BYTES: usize = 1024;

/// Argon2 runs at most this many at once, whatever the number of clients.
pub const MAX_CONCURRENT_HASHES: usize = 2;

/// The `settings` key of the public URL.
const PUBLIC_URL_KEY: &str = "public_url";

/// Where browsers reach the collector (umbrella spec §7.5): `https://`, or
/// `http://` to a loopback address, with no path, query or credentials.
/// Kept as its origin exactly as a browser serialises it in `Origin`
/// (lowercase scheme and host, no default port, no trailing slash), so the
/// two compare as plain strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl {
    origin: String,
    https: bool,
}

impl PublicUrl {
    pub fn parse(input: &str) -> Result<Self, String> {
        let url = url::Url::parse(input.trim()).map_err(|err| format!("public_url is not a URL: {err}"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("public_url must not hold credentials".into());
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err("public_url must be an origin only, with no path, query or fragment".into());
        }
        let https = match url.scheme() {
            "https" => true,
            "http" if is_loopback_host(&url) => false,
            "http" => return Err("public_url must be https://, or http:// to a loopback address".into()),
            _ => return Err("public_url must be https:// or http://".into()),
        };
        let origin = url.origin().ascii_serialization();
        Ok(Self { origin, https })
    }

    /// `scheme://host[:port]`, as a browser's `Origin` header has it.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether the session cookie is `Secure`: everywhere but loopback
    /// `http://` (kernel spec §3.2).
    pub fn is_https(&self) -> bool {
        self.https
    }
}

fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The outcome of `Operator::set_up`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    Done {
        owner_id: String,
    },
    /// There is an owner already.
    AlreadySetUp,
    /// Unknown, used or expired: one answer for all three.
    InvalidToken,
    /// The password or `public_url` is not acceptable (why); the token is
    /// not used up.
    Invalid(String),
}

struct SetupToken {
    hash: String,
    expires_at: i64,
}

pub struct Operator {
    conn: Mutex<Connection>,
    /// Loaded at open and replaced by setup: read on every browser request.
    public_url: RwLock<Option<PublicUrl>>,
    setup: Mutex<Option<SetupToken>>,
    hashing: tokio::sync::Semaphore,
    verifications: AtomicU64,
}

impl Operator {
    /// Open the kernel's tables in `hennery.db` (shared with the host
    /// registry and the sessions store).
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        db::migrate_component(&mut conn, schema::COMPONENT, schema::MIGRATIONS)?;
        let public_url = load_public_url(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            public_url: RwLock::new(public_url),
            setup: Mutex::new(None),
            hashing: tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES),
            verifications: AtomicU64::new(0),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("operator lock")
    }

    /// The owner's id, once setup has created it.
    pub fn owner_id(&self) -> Result<Option<String>> {
        Ok(self
            .conn()
            .query_row("SELECT id FROM owners ORDER BY created_at LIMIT 1", [], |r| r.get(0))
            .optional()?)
    }

    pub fn is_set_up(&self) -> Result<bool> {
        Ok(self.owner_id()?.is_some())
    }

    pub fn public_url(&self) -> Option<PublicUrl> {
        self.public_url.read().expect("public_url lock").clone()
    }

    /// A fresh setup token (64 hex characters), valid for
    /// `SETUP_TOKEN_TTL_SECS`, replacing any earlier one. `None` once there
    /// is an owner.
    pub fn issue_setup_token(&self, now: i64) -> Result<Option<String>> {
        if self.is_set_up()? {
            return Ok(None);
        }
        let token = hex::encode(random_bytes::<32>());
        *self.setup.lock().expect("setup lock") = Some(SetupToken {
            hash: sha256_hex(token.as_bytes()),
            expires_at: now + SETUP_TOKEN_TTL_SECS,
        });
        Ok(Some(token))
    }

    /// Create the owner with `password` and store `public_url` (kernel spec
    /// §3.1), if `token` is the live setup token. The token is used up only
    /// once the owner is committed. Hashes the password: call it on a
    /// blocking thread.
    pub fn set_up(&self, token: &str, password: &str, public_url: &str, now: i64) -> Result<SetupOutcome> {
        // Held throughout, so two setups cannot both pass the checks.
        let mut setup = self.setup.lock().expect("setup lock");
        if self.is_set_up()? {
            return Ok(SetupOutcome::AlreadySetUp);
        }
        let live = setup
            .as_ref()
            .is_some_and(|t| t.expires_at > now && t.hash == sha256_hex(token.as_bytes()));
        if !live {
            return Ok(SetupOutcome::InvalidToken);
        }
        if let Some(problem) = password_problem(password) {
            return Ok(SetupOutcome::Invalid(problem));
        }
        let public_url = match PublicUrl::parse(public_url) {
            Ok(url) => url,
            Err(problem) => return Ok(SetupOutcome::Invalid(problem)),
        };
        let phc = password_auth::generate_hash(password);
        let owner_id = format!("owner-{}", hex::encode(random_bytes::<8>()));
        {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO owners(id, created_at) VALUES (?1, ?2)",
                params![owner_id, now],
            )?;
            tx.execute(
                "INSERT INTO password_credentials(owner_id, phc, updated_at) VALUES (?1, ?2, ?3)",
                params![owner_id, phc, now],
            )?;
            tx.execute(
                "INSERT INTO settings(owner_id, key, value) VALUES (?1, ?2, ?3)",
                params![owner_id, PUBLIC_URL_KEY, public_url.origin()],
            )?;
            tx.commit()?;
        }
        *setup = None;
        *self.public_url.write().expect("public_url lock") = Some(public_url);
        Ok(SetupOutcome::Done { owner_id })
    }

    /// Whether `password` is the owner's. Before setup it is checked
    /// against a dummy hash, so the answer takes as long either way.
    /// Blocking: prefer `check_password`.
    pub fn verify_password(&self, password: &str) -> Result<bool> {
        self.verifications.fetch_add(1, Ordering::Relaxed);
        let phc: Option<String> = self
            .conn()
            .query_row("SELECT phc FROM password_credentials LIMIT 1", [], |r| r.get(0))
            .optional()?;
        let Some(phc) = phc else {
            let _ = password_auth::verify_password(password, dummy_hash());
            return Ok(false);
        };
        // An oversized password is still verified (and fails), so its
        // answer takes as long as any other.
        let password = if password.len() > MAX_PASSWORD_BYTES {
            ""
        } else {
            password
        };
        Ok(password_auth::verify_password(password, &phc).is_ok())
    }

    /// `verify_password` on a blocking thread, at most
    /// `MAX_CONCURRENT_HASHES` at once; others wait their turn.
    pub async fn check_password(self: &Arc<Self>, password: String) -> Result<bool> {
        let _permit = self
            .hashing
            .acquire()
            .await
            .context("the hashing semaphore is closed")?;
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.verify_password(&password)).await?
    }

    /// How many password checks have run: every login attempt that is not
    /// rate limited runs exactly one (kernel spec §3.2's constant-time
    /// failure path), which tests pin with this.
    pub fn verifications(&self) -> u64 {
        self.verifications.load(Ordering::Relaxed)
    }
}

fn load_public_url(conn: &Connection) -> Result<Option<PublicUrl>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1 LIMIT 1",
            [PUBLIC_URL_KEY],
            |r| r.get(0),
        )
        .optional()?;
    stored
        .map(|s| PublicUrl::parse(&s).map_err(|why| anyhow::anyhow!("the stored public_url is invalid: {why}")))
        .transpose()
}

/// Why `password` cannot be the owner's, if it cannot.
fn password_problem(password: &str) -> Option<String> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Some(format!("the password must be at least {MIN_PASSWORD_CHARS} characters"));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Some(format!("the password must be at most {MAX_PASSWORD_BYTES} bytes"));
    }
    None
}

/// A hash no password is checked against for real, so a check without an
/// owner costs one Argon2 verify like any other.
fn dummy_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| password_auth::generate_hash(hex::encode(random_bytes::<16>())))
}
```

Replace the whole of `crates/hennery-kernel/src/lib.rs` with:

```rust
//! Shared collector foundations (kernel spec): storage, request auth, and
//! the operator and their setup, and host identity and pairing.

pub mod auth;
pub mod db;
pub mod hosts;
pub mod lifecycle;
pub mod operator;
pub mod ratelimit;
mod schema;
pub mod secret;
```

- [ ] **Step 4: Update the lock file and run the new tests**

Run: `cargo build --workspace` (no `--locked`: it records `password-auth`, `password-hash`, `argon2` and `blake2` in `Cargo.lock`), then `cargo test -p hennery-kernel --locked`.
Expected: all pass, among them `a_restart_before_setup_invalidates_the_old_token`.

- [ ] **Step 5: Revert-probe the token and the URL rule**

Each probe is a one-line change. Rerun the named tests, see them fail, then restore the code.
1. Remove `t.expires_at > now && ` from `set_up`'s `live` check. Rerun `cargo test -p hennery-kernel --test operator --locked`. Expected: `a_setup_token_expires_after_an_hour` fails.
2. Remove ` && t.hash == sha256_hex(token.as_bytes())` from the same check. Expected: `a_setup_token_is_single_use_and_creates_one_owner` fails.
3. In `PublicUrl::parse`, change `"http" if is_loopback_host(&url) => false,` to `"http" => false,`. Expected: `public_urls_are_normalised_to_their_browser_origin` and `a_bad_password_or_public_url_is_refused_and_keeps_the_token` fail.

- [ ] **Step 6: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 400 tests pass; nothing to regenerate.

- [ ] **Step 7: Commit and push**

```bash
git add Cargo.toml Cargo.lock crates/hennery-kernel
git commit -m "feat(kernel): the operator, their password and the one-time setup token"
git push
```

### Task 2: Signed-in sessions with a sliding expiry and a step-up stamp

**Files:**
- Modify: `crates/hennery-kernel/src/operator.rs`
- Test: `crates/hennery-kernel/tests/auth_sessions.rs`

**Interfaces:**
- Consumes: Task 1's `Operator`, its connection and `owner_id()`.
- Produces (`hennery_kernel::operator`):
  - consts `SESSION_TTL_SECS = 2_592_000`, `SESSION_SLIDE_SECS = 60`, `STEP_UP_SECS = 300`, `SESSION_COOKIE = "hennery_session"`;
  - `struct Authenticated { session_id, owner_id: String, last_step_up_at: Option<i64>, expires_at: i64, slid: bool }`, with `stepped_up(now: i64) -> bool`;
  - `struct AuthSession { id, user_agent: String, created_at, last_seen_at: i64, last_step_up_at: Option<i64>, expires_at: i64 }`;
  - `Operator` methods:
    - `open_session(user_agent: &str, now) -> Result<Option<String>>`: the token, or `None` before setup;
    - `authenticate(token: &str, now) -> Result<Option<Authenticated>>`;
    - `step_up(session_id: &str, now) -> Result<bool>`;
    - `sessions(now) -> Result<Vec<AuthSession>>`;
    - `revoke_session(session_id: &str) -> Result<bool>`;
  - `fn session_cookie(token: &str, secure: bool) -> String`, `fn cleared_cookie(secure: bool) -> String`, and `fn session_token(&axum::http::HeaderMap) -> Option<&str>`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-kernel/tests/auth_sessions.rs`:

```rust
//! Signed-in sessions (kernel spec §3.2, §3.4): a random token in a cookie,
//! stored hashed, with a sliding 30-day expiry and a step-up stamp.

use axum::http::{HeaderMap, HeaderValue, header};
use hennery_kernel::operator::{
    Operator, SESSION_SLIDE_SECS, SESSION_TTL_SECS, STEP_UP_SECS, cleared_cookie, session_cookie, session_token,
};

const NOW: i64 = 1_800_000_000;

fn set_up(op: &Operator) {
    let token = op.issue_setup_token(NOW).unwrap().unwrap();
    op.set_up(&token, "correct horse battery", "https://hennery.example", NOW)
        .unwrap();
}

#[test]
fn there_is_no_session_before_setup() {
    let op = Operator::open_in_memory().unwrap();
    assert_eq!(op.open_session("browser", NOW).unwrap(), None);
}

#[test]
fn a_session_authenticates_until_it_expires_and_use_slides_its_expiry() {
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    assert_eq!(token.len(), 64);

    // Used again within the slide interval: nothing is written.
    let first = op.authenticate(&token, NOW + 1).unwrap().unwrap();
    assert_eq!((first.expires_at, first.slid), (NOW + SESSION_TTL_SECS, false));
    // Used later: the expiry moves to 30 days from then.
    let later = NOW + SESSION_SLIDE_SECS;
    let slid = op.authenticate(&token, later).unwrap().unwrap();
    assert_eq!((slid.expires_at, slid.slid), (later + SESSION_TTL_SECS, true));
    assert_eq!(slid.session_id, first.session_id);
    // Unused for 30 days after that, it is gone.
    assert!(op.authenticate(&token, later + SESSION_TTL_SECS - 1).unwrap().is_some());
    let idle = later + SESSION_TTL_SECS - 1 + SESSION_TTL_SECS;
    assert_eq!(op.authenticate(&token, idle).unwrap(), None);
    // A token that is not a session, or not even the right shape, is refused.
    assert_eq!(op.authenticate(&"0".repeat(64), NOW).unwrap(), None);
    assert_eq!(op.authenticate("", NOW).unwrap(), None);
}

#[test]
fn only_the_hash_of_a_session_token_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let op = Operator::open(&db).unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    let conn = rusqlite::Connection::open(&db).unwrap();
    let stored: String = conn
        .query_row("SELECT id_hash FROM auth_sessions", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, token);
    assert_eq!(stored, op.authenticate(&token, NOW).unwrap().unwrap().session_id);
}

#[test]
fn a_new_session_is_stepped_up_for_five_minutes_and_a_step_up_renews_it() {
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let token = op.open_session("browser", NOW).unwrap().unwrap();
    let session = op.authenticate(&token, NOW).unwrap().unwrap();
    assert!(session.stepped_up(NOW + STEP_UP_SECS - 1));
    assert!(!session.stepped_up(NOW + STEP_UP_SECS));

    let later = NOW + 3600;
    assert!(op.step_up(&session.session_id, later).unwrap());
    let renewed = op.authenticate(&token, later).unwrap().unwrap();
    assert!(renewed.stepped_up(later + STEP_UP_SECS - 1));
    assert!(!op.step_up(&"0".repeat(64), later).unwrap());
}

#[test]
fn sessions_are_listed_most_recent_first_and_a_revoked_one_is_gone() {
    let op = Operator::open_in_memory().unwrap();
    set_up(&op);
    let phone = op.open_session("phone\u{7}", NOW).unwrap().unwrap();
    let laptop = op.open_session(&"L".repeat(300), NOW + 10).unwrap().unwrap();
    let listed = op.sessions(NOW + 10).unwrap();
    assert_eq!(listed.len(), 2);
    // Control characters are dropped and the user agent is capped.
    assert_eq!(listed[0].user_agent, "L".repeat(256));
    assert_eq!(listed[1].user_agent, "phone");

    let phone_id = op.authenticate(&phone, NOW + 10).unwrap().unwrap().session_id;
    assert!(op.revoke_session(&phone_id).unwrap());
    assert!(!op.revoke_session(&phone_id).unwrap());
    assert_eq!(op.authenticate(&phone, NOW + 10).unwrap(), None);
    assert!(op.authenticate(&laptop, NOW + 10).unwrap().is_some());
    assert_eq!(op.sessions(NOW + 10).unwrap().len(), 1);
    // An expired session is not listed.
    assert!(op.sessions(NOW + 10 + SESSION_TTL_SECS).unwrap().is_empty());
}

#[test]
fn the_cookie_is_http_only_strict_and_secure_except_on_loopback() {
    assert_eq!(
        session_cookie("abc", true),
        "hennery_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure"
    );
    assert_eq!(
        session_cookie("abc", false),
        "hennery_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000"
    );
    assert_eq!(
        cleared_cookie(true),
        "hennery_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure"
    );
}

#[test]
fn the_session_token_is_found_among_other_cookies() {
    let mut headers = HeaderMap::new();
    assert_eq!(session_token(&headers), None);
    headers.append(header::COOKIE, HeaderValue::from_static("theme=dark; other_session=x"));
    assert_eq!(session_token(&headers), None);
    headers.append(header::COOKIE, HeaderValue::from_static("a=1;hennery_session=tok; b=2"));
    assert_eq!(session_token(&headers), Some("tok"));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-kernel --test auth_sessions`
Expected: FAIL to compile.
- `error[E0432]: unresolved imports hennery_kernel::operator::SESSION_SLIDE_SECS, …::SESSION_TTL_SECS, …::STEP_UP_SECS, …::cleared_cookie, …::session_cookie, …::session_token`;
- `error[E0599]: no method named authenticate found for struct Operator`, and the same for `open_session`, `revoke_session`, `sessions` and `step_up`.

- [ ] **Step 3: Sessions and the cookie**

Append to `crates/hennery-kernel/src/operator.rs`:

```rust
/// A signed-in session lives this long past its last use (kernel spec §3.2).
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// A session's expiry slides at most this often, so a busy client does
/// not write to the database on every request.
pub const SESSION_SLIDE_SECS: i64 = 60;

/// A password check is fresh enough for step-up this long (kernel spec
/// §3.4).
pub const STEP_UP_SECS: i64 = 5 * 60;

/// The session cookie's name (kernel spec §3.2).
pub const SESSION_COOKIE: &str = "hennery_session";

/// The longest `User-Agent` kept with a session, in characters.
const MAX_USER_AGENT: usize = 256;

/// A request's session, once its cookie checks out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authenticated {
    /// The session's id: the SHA-256 of its token, as stored.
    pub session_id: String,
    pub owner_id: String,
    pub last_step_up_at: Option<i64>,
    pub expires_at: i64,
    /// This request slid the expiry: the cookie is sent again with it.
    pub slid: bool,
}

impl Authenticated {
    /// Whether the last password check was within `STEP_UP_SECS`.
    pub fn stepped_up(&self, now: i64) -> bool {
        self.last_step_up_at.is_some_and(|at| now - at < STEP_UP_SECS)
    }
}

/// One signed-in session, as Settings lists them (kernel spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    pub id: String,
    pub user_agent: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub last_step_up_at: Option<i64>,
    pub expires_at: i64,
}

impl Operator {
    /// Open a session for the owner and return its token, the cookie's
    /// value. Only the token's hash is stored. The password was just
    /// checked, so the session starts stepped up. `None` before setup.
    pub fn open_session(&self, user_agent: &str, now: i64) -> Result<Option<String>> {
        let Some(owner_id) = self.owner_id()? else {
            return Ok(None);
        };
        let token = hex::encode(random_bytes::<32>());
        let user_agent: String = user_agent
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_USER_AGENT)
            .collect();
        let conn = self.conn();
        conn.execute("DELETE FROM auth_sessions WHERE expires_at <= ?1", [now])?;
        conn.execute(
            "INSERT INTO auth_sessions(id_hash, owner_id, user_agent, created_at, last_seen_at, last_step_up_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5)",
            params![
                sha256_hex(token.as_bytes()),
                owner_id,
                user_agent,
                now,
                now + SESSION_TTL_SECS
            ],
        )?;
        Ok(Some(token))
    }

    /// The live session `token` names, if any. Its expiry slides to
    /// `SESSION_TTL_SECS` from now, at most every `SESSION_SLIDE_SECS`.
    pub fn authenticate(&self, token: &str, now: i64) -> Result<Option<Authenticated>> {
        if token.len() != 64 {
            return Ok(None);
        }
        let id = sha256_hex(token.as_bytes());
        let conn = self.conn();
        let row: Option<(String, i64, Option<i64>, i64)> = conn
            .query_row(
                "SELECT owner_id, last_seen_at, last_step_up_at, expires_at FROM auth_sessions
                 WHERE id_hash = ?1 AND expires_at > ?2",
                params![id, now],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((owner_id, last_seen_at, last_step_up_at, mut expires_at)) = row else {
            return Ok(None);
        };
        let slid = now - last_seen_at >= SESSION_SLIDE_SECS;
        if slid {
            expires_at = now + SESSION_TTL_SECS;
            conn.execute(
                "UPDATE auth_sessions SET last_seen_at = ?2, expires_at = ?3 WHERE id_hash = ?1",
                params![id, now, expires_at],
            )?;
        }
        Ok(Some(Authenticated {
            session_id: id,
            owner_id,
            last_step_up_at,
            expires_at,
            slid,
        }))
    }

    /// Record a fresh password check on a session (kernel spec §3.4).
    /// Whether the session still exists.
    pub fn step_up(&self, session_id: &str, now: i64) -> Result<bool> {
        let changed = self.conn().execute(
            "UPDATE auth_sessions SET last_step_up_at = ?2 WHERE id_hash = ?1",
            params![session_id, now],
        )?;
        Ok(changed > 0)
    }

    /// Every live session, most recently used first.
    pub fn sessions(&self, now: i64) -> Result<Vec<AuthSession>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id_hash, user_agent, created_at, last_seen_at, last_step_up_at, expires_at
             FROM auth_sessions WHERE expires_at > ?1 ORDER BY last_seen_at DESC, id_hash",
        )?;
        let rows = stmt.query_map([now], |r| {
            Ok(AuthSession {
                id: r.get(0)?,
                user_agent: r.get(1)?,
                created_at: r.get(2)?,
                last_seen_at: r.get(3)?,
                last_step_up_at: r.get(4)?,
                expires_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// End a session. Whether there was one.
    pub fn revoke_session(&self, session_id: &str) -> Result<bool> {
        let changed = self
            .conn()
            .execute("DELETE FROM auth_sessions WHERE id_hash = ?1", [session_id])?;
        Ok(changed > 0)
    }
}

/// `Set-Cookie` for a session (kernel spec §3.2): `HttpOnly`,
/// `SameSite=Strict`, `Path=/`, for `SESSION_TTL_SECS`, and `Secure`
/// unless `public_url` is loopback `http://`.
pub fn session_cookie(token: &str, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_TTL_SECS}{secure}")
}

/// `Set-Cookie` that removes the session cookie.
pub fn cleared_cookie(secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0{secure}")
}

/// The session token in a request's `Cookie` headers, if there is one.
pub fn session_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE).then_some(value)
        })
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p hennery-kernel --test auth_sessions --locked`
Expected: all 7 pass.

- [ ] **Step 5: Revert-probe the expiry and the hashing**

Rerun `cargo test -p hennery-kernel --test auth_sessions --locked` after each change, see it fail, then restore the code.
1. In `authenticate`, change `WHERE id_hash = ?1 AND expires_at > ?2` to `WHERE id_hash = ?1 AND ?2 = ?2`. Expected: `a_session_authenticates_until_it_expires_and_use_slides_its_expiry` fails.
2. In `open_session`'s `INSERT`, store `token` in place of `sha256_hex(token.as_bytes())`. Expected: `only_the_hash_of_a_session_token_is_stored` fails (and so do the tests that authenticate).

- [ ] **Step 6: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 407 tests pass.

- [ ] **Step 7: Commit and push**

```bash
git add crates/hennery-kernel
git commit -m "feat(kernel): signed-in sessions with a sliding expiry and a step-up stamp"
git push
```

### Task 3: The one-time setup link and `POST /api/setup`

**Files:**
- Create: `crates/hennery-kernel/src/auth_api.rs`, `crates/hennery-kernel/src/setup_page.rs`
- Modify: `crates/hennery-kernel/src/operator.rs` (`announce_setup`, the `setup-url` file), `crates/hennery-kernel/src/lib.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, `crates/hennery-sessions/src/lib.rs`, `crates/hennery/src/main.rs`
- Modify (`AppState::new` gains the operator): `crates/hennery-testkit/tests/{auth,e2e,join,pairing,reconcile,ws_ingest_error}.rs`, `crates/hennery/tests/cli.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-kernel/tests/operator.rs`, `crates/hennery-testkit/tests/setup.rs`, `crates/hennery/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1 and 2 (`set_up`, `open_session`, `session_cookie`).
- Produces (`hennery_kernel::operator`):
  - `SETUP_URL_FILE = "setup-url"`;
  - `struct SetupLink { url: String, file: PathBuf }`;
  - `Operator::announce_setup(dir: &Path, base_url: &str, now) -> Result<Option<SetupLink>>`. `set_up` now also removes the file.
- Produces (`hennery_kernel::auth_api`):
  - `fn router(Arc<Operator>) -> Router`;
  - `pub(crate) fn error`, `internal`, `user_agent` and `with_cookie`.
- Produces (`hennery_proto::rest`): `SetupRequest { token, password, public_url: String }`, with a redacting `Debug`; and `SetupResponse { public_url: String }`.
- Produces (`hennery_sessions`): `AppState::new(store, hosts, operator: Operator, token: DevToken)` (the token goes in Task 5), and the field `operator: Arc<Operator>`. The router merges `auth_api::router`.
- Produces (HTTP):
  - `POST /api/setup`, answering 201 `SetupResponse` with `Set-Cookie`, or 400 `invalid`, 401 `invalid_setup_token`, 403 `origin_mismatch`, 409 `already_set_up` or 415;
  - `GET /setup` (the page, with the CSP) and `GET /setup.js` (its script). All three answer with `Referrer-Policy: no-referrer` and `Cache-Control: no-store`.
- Produces (`setup-url`): one line, `http://localhost:<port>/setup#<token>`.
- Produces (binary): once the collector listens, it writes `<data-dir>/setup-url`. It prints the link only to a terminal; otherwise it logs the file's path.

- [ ] **Step 1: Write the failing tests**

The kernel's tests cover the file; `setup.rs` covers the endpoint; the CLI test covers the announcement. Every harness now passes an `Operator` (on its database file, or in memory where the hosts are too).

In `crates/hennery-kernel/tests/operator.rs`, replace:

```rust

use hennery_kernel::operator::{MAX_PASSWORD_BYTES, Operator, PublicUrl, SETUP_TOKEN_TTL_SECS, SetupOutcome};
use std::sync::Arc;
```

with:

```rust

use hennery_kernel::operator::{
    MAX_PASSWORD_BYTES, Operator, PublicUrl, SETUP_TOKEN_TTL_SECS, SETUP_URL_FILE, SetupOutcome,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
```

Append to `crates/hennery-kernel/tests/operator.rs`:

```rust
fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Kernel spec §3.1: the link goes to `setup-url`, 0600, and is gone once
/// setup is done.
#[test]
fn the_setup_link_is_written_privately_and_removed_by_setup() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open(&dir.path().join("hennery.db")).unwrap();
    let link = op
        .announce_setup(dir.path(), "http://localhost:7117", NOW)
        .unwrap()
        .unwrap();
    assert_eq!(link.file, dir.path().join(SETUP_URL_FILE));
    assert_eq!(mode(&link.file), 0o600);
    assert_eq!(std::fs::read_to_string(&link.file).unwrap(), format!("{}\n", link.url));
    let token = link.url.strip_prefix("http://localhost:7117/setup#").unwrap();
    assert!(
        token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()),
        "{token}"
    );
    // No temporary file is left behind.
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        4,
        "hennery.db, -wal, -shm and setup-url"
    );

    assert!(matches!(
        op.set_up(token, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::Done { .. }
    ));
    assert!(!link.file.exists());
    // Set up: nothing is written, and a stale link is removed.
    std::fs::write(&link.file, "stale").unwrap();
    assert_eq!(
        op.announce_setup(dir.path(), "http://localhost:7117", NOW).unwrap(),
        None
    );
    assert!(!link.file.exists());
}

/// A second announcement replaces the first link, and only its token works.
#[test]
fn a_new_setup_link_replaces_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let op = Operator::open_in_memory().unwrap();
    let first = op
        .announce_setup(dir.path(), "http://localhost:1", NOW)
        .unwrap()
        .unwrap();
    let second = op
        .announce_setup(dir.path(), "http://localhost:1/", NOW)
        .unwrap()
        .unwrap();
    assert_ne!(first.url, second.url);
    assert_eq!(
        std::fs::read_to_string(&second.file).unwrap(),
        format!("{}\n", second.url)
    );
    let old = first.url.rsplit('#').next().unwrap();
    assert_eq!(
        op.set_up(old, PASSWORD, "https://hennery.example", NOW).unwrap(),
        SetupOutcome::InvalidToken
    );
}

/// A symlink planted at `setup-url` is replaced, never written through: the
/// token must not land in a file someone else chose.
#[test]
fn a_symlink_at_the_setup_link_is_replaced_and_its_target_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    std::fs::write(&target, "untouched").unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join(SETUP_URL_FILE)).unwrap();
    let op = Operator::open_in_memory().unwrap();
    let link = op
        .announce_setup(dir.path(), "http://localhost:1", NOW)
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
    assert!(!std::fs::symlink_metadata(&link.file).unwrap().file_type().is_symlink());
    assert_eq!(mode(&link.file), 0o600);
}
```

Create `crates/hennery-testkit/tests/setup.rs`:

```rust
//! The one-time setup over HTTP (kernel spec §3.1, §3.3): the token from
//! the setup link creates the owner and signs them in, once, from the
//! origin being stored as `public_url`.

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_proto::rest::{ApiError, SetupRequest, SetupResponse};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const PASSWORD: &str = "correct horse battery";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    token: String,
}

impl Collector {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
            DevToken::new("dev-token-for-tests").unwrap(),
        );
        let token = state
            .operator
            .issue_setup_token(hennery_kernel::secret::unix_now())
            .unwrap()
            .unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, token }
    }

    async fn setup(&self, origin: Option<&str>, token: &str, password: &str, public_url: &str) -> reqwest::Response {
        let mut req = reqwest::Client::new()
            .post(format!("http://{}/api/setup", self.addr))
            .json(&SetupRequest {
                token: token.into(),
                password: password.into(),
                public_url: public_url.into(),
            });
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        req.send().await.unwrap()
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

fn session_cookie(resp: &reqwest::Response) -> String {
    resp.headers()["set-cookie"].to_str().unwrap().to_string()
}

#[tokio::test]
async fn setup_creates_the_owner_signs_them_in_and_happens_once() {
    let c = Collector::start().await;
    let origin = "https://hennery.example";
    let resp = c
        .setup(Some(origin), &c.token, PASSWORD, "https://Hennery.Example:443/")
        .await;
    assert_eq!(resp.status(), 201);
    let cookie = session_cookie(&resp);
    assert!(
        cookie.ends_with("; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure"),
        "{cookie}"
    );
    let body: SetupResponse = resp.json().await.unwrap();
    assert_eq!(body.public_url, origin);
    // The cookie is a live session, stepped up by the password just set.
    let token = cookie
        .strip_prefix("hennery_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let session = c
        .state
        .operator
        .authenticate(token, hennery_kernel::secret::unix_now())
        .unwrap()
        .unwrap();
    assert!(session.stepped_up(hennery_kernel::secret::unix_now()));
    assert_eq!(c.state.operator.public_url().unwrap().origin(), origin);

    let again = c.setup(Some(origin), &c.token, PASSWORD, origin).await;
    assert_eq!(code_of(again).await, (409, "already_set_up".into()));
}

/// Kernel spec §3.3: setup is a state-changing browser route. Before there
/// is a `public_url`, `Origin` must be the one being stored.
#[tokio::test]
async fn setup_needs_the_origin_of_the_public_url_it_stores() {
    let c = Collector::start().await;
    let url = "https://hennery.example";
    for origin in [
        None,
        Some("https://evil.example"),
        Some("null"),
        Some("http://hennery.example"),
    ] {
        let resp = c.setup(origin, &c.token, PASSWORD, url).await;
        assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()), "{origin:?}");
    }
    // None of that used the token up.
    assert_eq!(c.setup(Some(url), &c.token, PASSWORD, url).await.status(), 201);
}

#[tokio::test]
async fn a_wrong_token_bad_input_or_a_form_post_is_refused_and_keeps_the_token() {
    let c = Collector::start().await;
    let url = "http://127.0.0.1:7117";
    let wrong = c.setup(Some(url), &"0".repeat(64), PASSWORD, url).await;
    assert_eq!(code_of(wrong).await, (401, "invalid_setup_token".into()));
    let short = c.setup(Some(url), &c.token, "short", url).await;
    assert_eq!(code_of(short).await, (400, "invalid".into()));
    let remote_http = c
        .setup(
            Some("http://hennery.example"),
            &c.token,
            PASSWORD,
            "http://hennery.example",
        )
        .await;
    assert_eq!(code_of(remote_http).await, (400, "invalid".into()));
    // A form post (what a cross-site page can send without a preflight) is
    // not JSON and is refused before anything is looked at.
    let form = reqwest::Client::new()
        .post(format!("http://{}/api/setup", c.addr))
        .header("origin", url)
        .header("content-type", "text/plain")
        .body(serde_json::json!({"token": c.token, "password": PASSWORD, "public_url": url}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), 415);

    let resp = c.setup(Some(url), &c.token, PASSWORD, url).await;
    assert_eq!(resp.status(), 201);
    // Loopback `http://`: the cookie cannot be `Secure`.
    let cookie = session_cookie(&resp);
    assert!(!cookie.contains("Secure"), "{cookie}");
}

fn assert_private(resp: &reqwest::Response) {
    assert_eq!(resp.headers()["referrer-policy"], "no-referrer");
    assert_eq!(resp.headers()["cache-control"], "no-store");
}

/// Every setup response is uncached and sends no `Referer` onwards, the
/// refusals as well as the success.
#[tokio::test]
async fn setup_responses_are_never_cached_nor_referred() {
    let c = Collector::start().await;
    let url = "https://hennery.example";
    let refused = c.setup(None, &c.token, PASSWORD, url).await;
    assert_private(&refused);
    let done = c.setup(Some(url), &c.token, PASSWORD, url).await;
    assert_eq!(done.status(), 201);
    assert_private(&done);
}

/// 3b decision 16: the link is `/setup#<token>`. The page and its script
/// are static; the script reads the token from the fragment and sends it
/// only in the `POST /api/setup` body. The page runs no inline script.
#[tokio::test]
async fn the_setup_page_is_static_and_reads_the_token_from_the_fragment() {
    let c = Collector::start().await;
    let page = reqwest::get(format!("http://{}/setup", c.addr)).await.unwrap();
    assert_eq!(page.status(), 200);
    assert_private(&page);
    let csp = page.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(
        csp.starts_with("script-src 'self';") && csp.contains("frame-ancestors 'none'"),
        "{csp}"
    );
    let html = page.text().await.unwrap();
    assert!(html.contains(r#"<script src="/setup.js" defer></script>"#), "{html}");
    assert_eq!(html.matches("<script").count(), 1, "an inline script: {html}");

    let script = reqwest::get(format!("http://{}/setup.js", c.addr)).await.unwrap();
    assert_eq!(script.status(), 200);
    assert_private(&script);
    assert!(
        script.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    let js = script.text().await.unwrap();
    assert!(
        js.contains("location.hash") && js.contains(r#"fetch("/api/setup""#),
        "{js}"
    );
}

/// A logged request must never show the password.
#[test]
fn a_setup_request_does_not_show_its_password_in_debug() {
    let req = SetupRequest {
        token: "t".into(),
        password: PASSWORD.into(),
        public_url: "https://hennery.example".into(),
    };
    let shown = format!("{req:?}");
    assert!(
        !shown.contains(PASSWORD) && shown.contains("hennery.example"),
        "{shown}"
    );
}
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::frames::{CollectorFrame, HostFrame};
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{CollectorFrame, HostFrame};
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

with:

```rust
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::rest::{EventDto, HostItem, PromptResponse, StartSessionResponse};
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::rest::{EventDto, HostItem, PromptResponse, StartSessionResponse};
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        pair_host(&hosts);
        let mut state = AppState::new(Store::open(db).unwrap(), hosts, DevToken::new(TOKEN).unwrap());
        state.offline_threshold = offline;
```

with:

```rust
        pair_host(&hosts);
        let mut state = AppState::new(
            Store::open(db).unwrap(),
            hosts,
            Operator::open(db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
        state.offline_threshold = offline;
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HelloCheck, Hosts};
use hennery_proto::PROTOCOL_VERSION;
```

with:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HelloCheck, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::PROTOCOL_VERSION;
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
            Hosts::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

with:

```rust
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
use hennery_proto::rest::{ApiError, EnrollRequest, EnrollResponse, PairingCodeResponse};
```

with:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::rest::{ApiError, EnrollRequest, EnrollResponse, PairingCodeResponse};
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust
            Hosts::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

with:

```rust
            Hosts::open(&db).unwrap(),
            Operator::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionBody};
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            hosts,
            DevToken::new(TOKEN).unwrap(),
```

with:

```rust
            hosts,
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame, SessionBody};
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame, SessionBody};
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, paired_hosts(), DevToken::new(TOKEN).unwrap());
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
```

with:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(
        store,
        paired_hosts(),
        Operator::open_in_memory().unwrap(),
        DevToken::new(TOKEN).unwrap(),
    );
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust

    let state = AppState::new(store, paired_hosts(), DevToken::new(TOKEN).unwrap());
    let shutdown = state.shutdown.clone();
```

with:

```rust

    let state = AppState::new(
        store,
        paired_hosts(),
        Operator::open_in_memory().unwrap(),
        DevToken::new(TOKEN).unwrap(),
    );
    let shutdown = state.shutdown.clone();
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        let token = hennery_kernel::auth::DevToken::new("dev-token-for-tests").unwrap();
        let state = hennery_sessions::AppState::new(store, hosts, token);
        let code = state
```

with:

```rust
        let token = hennery_kernel::auth::DevToken::new("dev-token-for-tests").unwrap();
        let operator = hennery_kernel::operator::Operator::open(&db).unwrap();
        let state = hennery_sessions::AppState::new(store, hosts, operator, token);
        let code = state
```

Append to `crates/hennery/tests/cli.rs`:

```rust
/// Kernel spec §3.1: a collector that is not set up writes its one-time
/// setup link to `setup-url` (0600, under `umask 022` too) and, its output
/// not being a terminal, logs only that file's path: the token itself must
/// never reach a log collector.
#[test]
fn an_unset_collector_writes_its_setup_link_to_a_private_file_and_never_to_its_output() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-setup-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");

    let mut collector = collector_under_umask_022(&listen, &data, &log);
    let file = data.join("setup-url");
    assert_eq!(mode_of(&file), 0o600);
    let url = std::fs::read_to_string(&file).unwrap();
    let port = listen.rsplit(':').next().unwrap();
    let token = url
        .trim_end()
        .strip_prefix(&format!("http://localhost:{port}/setup#"))
        .unwrap_or_else(|| panic!("{url}"));
    assert_eq!(token.len(), 64, "{url}");
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());

    let stdout = std::fs::read_to_string(&log).unwrap();
    let stderr = std::fs::read_to_string(log.with_extension("err")).unwrap();
    assert!(
        !stdout.contains(token) && !stderr.contains(token),
        "the setup token was logged"
    );
    assert!(stdout.contains(&file.display().to_string()), "{stdout}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-kernel --test operator; cargo test -p hennery-testkit --test setup`
Expected: FAIL to compile.
- `error[E0432]: unresolved import hennery_kernel::operator::SETUP_URL_FILE`;
- `error[E0432]: unresolved imports hennery_proto::rest::SetupRequest, hennery_proto::rest::SetupResponse`;
- `error[E0599]: no method named announce_setup found for struct Operator`;
- `error[E0061]: this function takes 3 arguments but 4 arguments were supplied` (`AppState::new`);
- `error[E0609]: no field operator on type AppState`.

- [ ] **Step 3: The link, the endpoint and the announcement**

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
```

with:

```rust
use rusqlite::{Connection, OptionalExtension, params};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
pub const MAX_CONCURRENT_HASHES: usize = 2;

```

with:

```rust
pub const MAX_CONCURRENT_HASHES: usize = 2;

/// The file in the data directory that holds the setup link until setup
/// (kernel spec §1, §3.1).
pub const SETUP_URL_FILE: &str = "setup-url";

```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust

pub struct Operator {
```

with:

```rust

/// Where the setup link was written (`Operator::announce_setup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupLink {
    /// `<base>/setup#<token>`: the token in the fragment (3b decision 16).
    pub url: String,
    /// The 0600 file holding `url`.
    pub file: PathBuf,
}

pub struct Operator {
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    setup: Mutex<Option<SetupToken>>,
    hashing: tokio::sync::Semaphore,
```

with:

```rust
    setup: Mutex<Option<SetupToken>>,
    /// The `setup-url` file, removed once setup is done.
    setup_file: Mutex<Option<PathBuf>>,
    hashing: tokio::sync::Semaphore,
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            setup: Mutex::new(None),
            hashing: tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES),
```

with:

```rust
            setup: Mutex::new(None),
            setup_file: Mutex::new(None),
            hashing: tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES),
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        });
        Ok(Some(token))
    }

```

with:

```rust
        });
        Ok(Some(token))
    }

    /// Before setup: issue a fresh setup token and write its link,
    /// `<base_url>/setup#<token>`, to `dir/setup-url` (kernel spec §3.1).
    /// The file is created 0600 under a temporary name and renamed into
    /// place, so an existing `setup-url` (even a symlink) is replaced, never
    /// written through. Once set up: remove a stale `setup-url` and return
    /// `None`.
    pub fn announce_setup(&self, dir: &Path, base_url: &str, now: i64) -> Result<Option<SetupLink>> {
        let file = dir.join(SETUP_URL_FILE);
        let Some(token) = self.issue_setup_token(now)? else {
            remove_setup_file(&file);
            return Ok(None);
        };
        let url = format!("{}/setup#{token}", base_url.trim_end_matches('/'));
        let temp = dir.join(format!(".{SETUP_URL_FILE}.{}.tmp", hex::encode(random_bytes::<8>())));
        let written = (|| {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            writeln!(out, "{url}")?;
            out.sync_all()?;
            std::fs::rename(&temp, &file)
        })();
        if let Err(err) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(err).with_context(|| format!("write {}", file.display()));
        }
        *self.setup_file.lock().expect("setup file lock") = Some(file.clone());
        Ok(Some(SetupLink { url, file }))
    }

```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        *setup = None;
        *self.public_url.write().expect("public_url lock") = Some(public_url);
```

with:

```rust
        *setup = None;
        if let Some(file) = self.setup_file.lock().expect("setup file lock").take() {
            remove_setup_file(&file);
        }
        *self.public_url.write().expect("public_url lock") = Some(public_url);
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
        self.verifications.load(Ordering::Relaxed)
    }
```

with:

```rust
        self.verifications.load(Ordering::Relaxed)
    }
}

fn remove_setup_file(file: &Path) {
    match std::fs::remove_file(file) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(file = %file.display(), error = %err, "could not remove the setup link"),
    }
```

Create `crates/hennery-kernel/src/auth_api.rs`:

```rust
//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup.

use crate::operator::{Operator, PublicUrl, SetupOutcome, session_cookie};
use crate::secret::unix_now;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, SetupRequest, SetupResponse};
use std::sync::Arc;

/// The operator auth routes, each with its own `Origin` rule (kernel spec
/// §3.3).
pub fn router(operator: Arc<Operator>) -> Router {
    let private = || middleware::map_response(crate::setup_page::private_headers);
    Router::new()
        .route("/api/setup", post(setup).layer(private()))
        .route("/setup", get(crate::setup_page::page).layer(private()))
        .route("/setup.js", get(crate::setup_page::script).layer(private()))
        .with_state(operator)
}

pub(crate) fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            code: code.into(),
            message: message.into(),
            session_id: None,
        }),
    )
        .into_response()
}

pub(crate) fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

/// The request's `User-Agent`, for the session list.
pub(crate) fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A response that sets `cookie`.
pub(crate) fn with_cookie(mut response: Response, cookie: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

/// `POST /api/setup`: 201, the owner signed in. There is no `public_url`
/// yet to check `Origin` against, so it must be the origin of the
/// `public_url` being stored: the browser that sets hennery up is the one
/// that can use it afterwards. The token is what authenticates.
async fn setup(State(operator): State<Arc<Operator>>, headers: HeaderMap, Json(req): Json<SetupRequest>) -> Response {
    let public_url = match PublicUrl::parse(&req.public_url) {
        Ok(url) => url,
        Err(why) => return error(StatusCode::BAD_REQUEST, "invalid", why),
    };
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin != Some(public_url.origin()) {
        tracing::debug!(expected = %public_url.origin(), received = ?origin, "setup refused: origin_mismatch");
        return error(
            StatusCode::FORBIDDEN,
            "origin_mismatch",
            "Origin must be the public_url being set up",
        );
    }
    let op = operator.clone();
    let outcome =
        tokio::task::spawn_blocking(move || op.set_up(&req.token, &req.password, &req.public_url, unix_now())).await;
    match outcome {
        Ok(Ok(SetupOutcome::Done { owner_id })) => {
            tracing::info!(%owner_id, public_url = %public_url.origin(), "hennery set up");
        }
        Ok(Ok(SetupOutcome::AlreadySetUp)) => {
            return error(StatusCode::CONFLICT, "already_set_up", "hennery is set up already");
        }
        Ok(Ok(SetupOutcome::InvalidToken)) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "invalid_setup_token",
                "the setup link is unknown, used or expired; restart the collector for a new one",
            );
        }
        Ok(Ok(SetupOutcome::Invalid(why))) => return error(StatusCode::BAD_REQUEST, "invalid", why),
        Ok(Err(err)) => return internal(err),
        Err(err) => return internal(err.into()),
    }
    let token = match operator.open_session(&user_agent(&headers), unix_now()) {
        Ok(Some(token)) => token,
        Ok(None) => return internal(anyhow::anyhow!("no owner right after setup")),
        Err(err) => return internal(err),
    };
    let response = (
        StatusCode::CREATED,
        Json(SetupResponse {
            public_url: public_url.origin().to_string(),
        }),
    )
        .into_response();
    with_cookie(response, &session_cookie(&token, public_url.is_https()))
}
```

Create `crates/hennery-kernel/src/setup_page.rs`:

```rust
//! The page a setup link opens (kernel spec §3.1), until the frontend
//! replaces it. The link is `…/setup#<token>`: the token is in the
//! fragment, which a browser never sends, so it reaches no server log,
//! proxy log or `Referer` (3b decision 16). The page's script reads it
//! from `location.hash` and sends it in the `POST /api/setup` body.
//!
//! The page and its script are static and hold no data, so they sit
//! outside the browser rules and the session cookie (3b decision 13). The
//! script is a file of its own, not inline, so the page's
//! `Content-Security-Policy` can be kernel spec §7.2's without a hash.

use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};

const PAGE: &str = r#"<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Set up hennery</title>
<h1>Set up hennery</h1>
<form id="setup">
  <p><label>Password (at least 12 characters)<br><input id="password" type="password" minlength="12" required autocomplete="new-password"></label></p>
  <p><label>Public URL (where your browser reaches hennery)<br><input id="public_url" type="url" required></label></p>
  <p><button type="submit">Set up</button></p>
</form>
<p id="result" role="status"></p>
<script src="/setup.js" defer></script>
</html>
"#;

const SCRIPT: &str = r#"// hennery setup: the token is in the fragment and leaves the browser only
// in this request's body.
const token = location.hash.slice(1);
history.replaceState(null, "", location.pathname);
const result = document.getElementById("result");
document.getElementById("public_url").value = location.origin;
if (!token) {
  result.textContent = "This page needs the setup link the collector wrote to its setup-url file.";
}
document.getElementById("setup").addEventListener("submit", async (event) => {
  event.preventDefault();
  const response = await fetch("/api/setup", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      token,
      password: document.getElementById("password").value,
      public_url: document.getElementById("public_url").value,
    }),
  });
  if (response.ok) {
    result.textContent = "hennery is set up, and you are signed in.";
  } else {
    const body = await response.json().catch(() => ({}));
    result.textContent = "Setup failed: " + (body.message || response.status);
  }
});
"#;

/// Kernel spec §7.2's policy, without the theme bootstrap this page has not.
const CSP: &str =
    "script-src 'self'; img-src 'self' data: blob:; object-src 'none'; frame-ancestors 'none'; base-uri 'none'";

/// `GET /setup`.
pub(crate) async fn page() -> Response {
    let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], PAGE).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
    response
}

/// `GET /setup.js`.
pub(crate) async fn script() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8")], SCRIPT).into_response()
}

/// Every setup response, the page and the API alike: never cached, and
/// never a `Referer` onwards.
pub(crate) async fn private_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
pub mod auth;
pub mod db;
```

with:

```rust
pub mod auth;
pub mod auth_api;
pub mod db;
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
pub mod secret;
```

with:

```rust
pub mod secret;
mod setup_page;
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// `POST /api/setup` (kernel spec §3.1): the one-time owner setup, with the
/// token from the setup link. `Debug` leaves the password out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetupRequest {
    pub token: String,
    pub password: String,
    /// `https://…`, or `http://` to a loopback address; an origin only.
    pub public_url: String,
}

impl std::fmt::Debug for SetupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupRequest")
            .field("public_url", &self.public_url)
            .finish_non_exhaustive()
    }
}

/// 201 to a setup: the owner is created and signed in (the session cookie
/// is set), and `public_url` is stored as this origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SetupResponse {
    pub public_url: String,
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::EnrollResponse,
        rest::HostItem,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::EnrollResponse,
        rest::HostItem,
        rest::SetupRequest,
        rest::SetupResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::HostItem,
    );
```

with:

```rust
        rest::HostItem,
        rest::SetupRequest,
        rest::SetupResponse,
    );
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_kernel::ratelimit::{Limiter, Policy};
```

with:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::ratelimit::{Limiter, Policy};
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    pub hosts: Arc<Hosts>,
    /// Wrong pairing codes per client address (kernel spec §4.1).
```

with:

```rust
    pub hosts: Arc<Hosts>,
    /// The owner, their sessions and setup (kernel spec §3).
    pub operator: Arc<Operator>,
    /// Wrong pairing codes per client address (kernel spec §4.1).
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
impl AppState {
    pub fn new(store: store::Store, hosts: Hosts, token: DevToken) -> Self {
        Self {
            store: Arc::new(store),
            hosts: Arc::new(hosts),
            enroll_limiter: Arc::new(Limiter::new(Policy::ENROLL)),
```

with:

```rust
impl AppState {
    pub fn new(store: store::Store, hosts: Hosts, operator: Operator, token: DevToken) -> Self {
        Self {
            store: Arc::new(store),
            hosts: Arc::new(hosts),
            operator: Arc::new(operator),
            enroll_limiter: Arc::new(Limiter::new(Policy::ENROLL)),
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust

/// Every session and host route plus the host WebSocket. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`: enrollment reads
```

with:

```rust

/// Every session, host and operator route plus the host WebSocket. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`: enrollment reads
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
        .merge(hosts::router(state.clone()))
        .merge(ws::router(state))
```

with:

```rust
        .merge(hosts::router(state.clone()))
        .merge(hennery_kernel::auth_api::router(state.operator.clone()))
        .merge(ws::router(state))
```

In `crates/hennery/src/main.rs`, replace:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_sessions::{AppState, store::Store};
```

with:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, SetupLink};
use hennery_sessions::{AppState, store::Store};
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let hosts = Hosts::open(&db)?;
    let mut state = AppState::new(store, hosts, token);
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
```

with:

```rust
    let hosts = Hosts::open(&db)?;
    let operator = Operator::open(&db)?;
    let mut state = AppState::new(store, hosts, operator, token);
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .with_context(|| format!("bind {}", args.listen))?;
    tracing::info!(address = %listener.local_addr()?, "collector listening");
    if let Some(fd) = args.pairing_code_fd {
```

with:

```rust
        .with_context(|| format!("bind {}", args.listen))?;
    let address = listener.local_addr()?;
    tracing::info!(%address, "collector listening");
    // Only once listening: the link names the port (kernel spec §3.1).
    let base_url = format!("http://localhost:{}", address.port());
    if let Some(link) = state
        .operator
        .announce_setup(&args.data_dir, &base_url, hennery_kernel::secret::unix_now())?
    {
        announce_setup(&link);
    }
    if let Some(fd) = args.pairing_code_fd {
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .await?;
    Ok(())
}

```

with:

```rust
        .await?;
    Ok(())
}

/// Tell the operator where the setup link is (kernel spec §3.1): the link
/// itself only to a terminal, so the token never lands in a log collector;
/// otherwise only the path of the file that holds it.
fn announce_setup(link: &SetupLink) {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        println!(
            "hennery is not set up yet. Open this link within the hour to set it up:\n  {}",
            link.url
        );
    } else {
        tracing::info!(
            file = %link.file.display(),
            "hennery is not set up yet; the one-time setup link (valid for an hour) is in this file"
        );
    }
}

```

- [ ] **Step 4: Regenerate and run the new tests**

Run: `cargo run -p hennery-proto --bin gen`
Expected: `wrote schema/hennery-protocol.schema.json`, `wrote web/src/generated/protocol.ts`.

Run: `cargo test -p hennery-kernel --test operator --locked && cargo test -p hennery-testkit --test setup --locked && cargo test -p hennery --test cli an_unset_collector --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probe the terminal check, setup's `Origin` rule, the file, the headers and `Debug`**

1. In `announce_setup` (`main.rs`), change `if std::io::stdout().is_terminal() {` to `if std::io::stdout().is_terminal() || true {`, and rerun `cargo test -p hennery --test cli an_unset_collector --locked`. Expected: it fails with `the setup token was logged`. Restore the code.
2. In `auth_api::setup`, change `if origin != Some(public_url.origin()) {` to `if false && origin != Some(public_url.origin()) {`, and rerun `cargo test -p hennery-testkit --test setup --locked`. Expected: `setup_needs_the_origin_of_the_public_url_it_stores` fails. Restore the code.
3. In `announce_setup` (`operator.rs`), open `&file` itself with `.create(true).truncate(true)` in place of `&temp` with `.create_new(true)`, so a symlink there is followed. Rerun `cargo test -p hennery-kernel --test operator --locked`. Expected: `a_symlink_at_the_setup_link_is_replaced_and_its_target_left_alone` fails. Restore the code.
4. Change the file's `.mode(0o600)` to `.mode(0o644)`. Expected: `the_setup_link_is_written_privately_and_removed_by_setup` fails. Restore the code.
5. Remove the three lines in `set_up` that take `setup_file` and call `remove_setup_file`. Expected: the same test fails. Restore the code.
6. In `auth_api::router`, change `.route("/api/setup", post(setup).layer(private()))` to `.route("/api/setup", post(setup))`. Rerun `cargo test -p hennery-testkit --test setup --locked`. Expected: `setup_responses_are_never_cached_nor_referred` fails. Restore the code.
7. In `SetupRequest`'s `Debug`, add `.field("password", &self.password)`. Expected: `a_setup_request_does_not_show_its_password_in_debug` fails. Restore the code.

- [ ] **Step 6: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 417 tests pass.

- [ ] **Step 7: Commit and push**

```bash
git add crates schema web/src/generated
git commit -m "feat(kernel): the one-time setup link and POST /api/setup"
git push
```

### Task 4: Login and logout, rate limited, behind the browser `Origin` rules

**Files:**
- Create: `crates/hennery-kernel/src/origin.rs`
- Modify: `crates/hennery-kernel/src/auth_api.rs`, `crates/hennery-kernel/src/operator.rs` (`login_limiter`), `crates/hennery-kernel/src/ratelimit.rs` (`Policy::LOGIN`, loopback), `crates/hennery-kernel/src/lib.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-testkit/tests/login.rs`; a unit test in `ratelimit.rs`

**Interfaces:**
- Consumes: Tasks 1–3 (`check_password`, `open_session`, `session_token`, `cleared_cookie`, the `auth_api` helpers).
- Produces (`hennery_kernel::ratelimit`): `Policy::LOGIN { free_failures: 5, window: 60 s, first_lockout: 60 s, max_lockout: 1 h }`. `key` maps `127.0.0.0/8` to `127.0.0.1` and leaves `::1` as it is. Loopback gets its own entry past capacity.
- Produces (`hennery_kernel::operator`): the field `Operator::login_limiter: Limiter`.
- Produces (`hennery_kernel::origin`): `async fn browser_rules(State<Arc<Operator>>, Request, Next) -> Response`, which answers 403 `setup_required`, `origin_mismatch` or `cross_site`, or 415 `unsupported_media_type`.
- Produces (`hennery_kernel::auth_api`): `pub const MAX_BODY_BYTES: usize = 16 * 1024`, applied to every route of `router`; `pub(crate) fn rate_limited(Duration, &str) -> Response` and `pub(crate) fn secure_cookies(&Operator) -> bool`.
- Produces (`hennery_proto::rest`): `LoginRequest { password: String }`, with a redacting `Debug`.
- Produces (HTTP), both behind the browser rules:
  - `POST /api/auth/login`: 204 with `Set-Cookie`, 401 `invalid_password`, or 429 `rate_limited` with `Retry-After`;
  - `POST /api/auth/logout`: 204, clearing the cookie.

- [ ] **Step 1: Write the failing tests**

The limiter's new unit test is in Step 3, with the code it tests.

Create `crates/hennery-testkit/tests/login.rs`:

```rust
//! Login and logout (kernel spec §3.2, §3.3, §11): the owner's password
//! opens a session, rate limited per client address with one password
//! check per attempt, and only from the `public_url`'s origin.

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{ApiError, LoginRequest};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const PASSWORD: &str = "correct horse battery";
const ORIGIN: &str = "https://hennery.example";

struct Collector {
    addr: SocketAddr,
    state: AppState,
}

impl Collector {
    async fn start(set_up: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
            DevToken::new("dev-token-for-tests").unwrap(),
        );
        if set_up {
            let token = state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
            state.operator.set_up(&token, PASSWORD, ORIGIN, unix_now()).unwrap();
        }
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("http://{}{path}", self.addr))
            .header("origin", ORIGIN)
    }

    async fn login(&self, password: &str) -> reqwest::Response {
        self.post("/api/auth/login")
            .json(&LoginRequest {
                password: password.into(),
            })
            .send()
            .await
            .unwrap()
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (status, resp.json::<ApiError>().await.unwrap().code)
}

/// The session token a response's `Set-Cookie` carries.
fn cookie_token(resp: &reqwest::Response) -> String {
    let cookie = resp.headers()["set-cookie"].to_str().unwrap();
    cookie
        .strip_prefix("hennery_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_owners_password_opens_a_session_and_logout_ends_it() {
    let c = Collector::start(true).await;
    let resp = c.login(PASSWORD).await;
    assert_eq!(resp.status(), 204);
    assert!(
        resp.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .ends_with("; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000; Secure")
    );
    let token = cookie_token(&resp);
    assert!(c.state.operator.authenticate(&token, unix_now()).unwrap().is_some());

    let out = c
        .post("/api/auth/logout")
        .header("cookie", format!("hennery_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(out.status(), 204);
    assert_eq!(cookie_token(&out), "");
    assert!(c.state.operator.authenticate(&token, unix_now()).unwrap().is_none());
    // Logging out again, or without a session, is harmless.
    let again = c.post("/api/auth/logout").send().await.unwrap();
    assert_eq!(again.status(), 204);
}

/// Kernel spec §3.2: 5 wrong passwords a minute per address, then backoff,
/// and every attempt that gets in runs exactly one password check (the
/// constant-time failure path); a refused one runs none, and a right
/// password is refused too while the address is locked out.
#[tokio::test]
async fn wrong_passwords_lock_the_address_out_and_each_attempt_checks_once() {
    let c = Collector::start(true).await;
    for n in 1..=5 {
        assert_eq!(
            code_of(c.login("wrong password").await).await,
            (401, "invalid_password".into())
        );
        assert_eq!(c.state.operator.verifications(), n);
    }
    let locked = c.login(PASSWORD).await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(code_of(locked).await, (429, "rate_limited".into()));
    assert_eq!(
        c.state.operator.verifications(),
        5,
        "a locked-out attempt checked the password"
    );
}

#[tokio::test]
async fn a_right_password_clears_the_count() {
    let c = Collector::start(true).await;
    for _ in 0..4 {
        c.login("wrong password").await;
    }
    assert_eq!(c.login(PASSWORD).await.status(), 204);
    for _ in 0..4 {
        assert_eq!(c.login("wrong password").await.status(), 401);
    }
    assert_eq!(c.login(PASSWORD).await.status(), 204);
}

/// Kernel spec §3.3 on the login route itself: from the `public_url`'s
/// origin only, as JSON, and not before setup.
#[tokio::test]
async fn login_needs_the_public_urls_origin_json_and_setup() {
    let c = Collector::start(true).await;
    let body = serde_json::json!({ "password": PASSWORD }).to_string();
    let url = format!("http://{}/api/auth/login", c.addr);
    let client = reqwest::Client::new();
    let missing = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(missing).await, (403, "origin_mismatch".into()));
    for origin in [
        "https://evil.example",
        "http://hennery.example",
        "https://hennery.example:444",
    ] {
        let resp = client
            .post(&url)
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(resp).await, (403, "origin_mismatch".into()), "{origin}");
    }
    for content_type in [Some("text/plain"), Some("application/x-www-form-urlencoded"), None] {
        let mut req = c.post("/api/auth/login").body(body.clone());
        if let Some(content_type) = content_type {
            req = req.header("content-type", content_type);
        }
        let resp = req.send().await.unwrap();
        assert_eq!(
            code_of(resp).await,
            (415, "unsupported_media_type".into()),
            "{content_type:?}"
        );
    }
    // None of those reached the password check.
    assert_eq!(c.state.operator.verifications(), 0);

    let unset = Collector::start(false).await;
    assert_eq!(
        code_of(unset.login(PASSWORD).await).await,
        (403, "setup_required".into())
    );
}

/// A body far larger than any password is refused with 413 before it is
/// parsed, on setup and on login alike.
#[tokio::test]
async fn an_oversized_body_is_refused_before_it_is_parsed() {
    let unset = Collector::start(false).await;
    let token = unset.state.operator.issue_setup_token(unix_now()).unwrap().unwrap();
    let huge = "x".repeat(hennery_kernel::auth_api::MAX_BODY_BYTES + 1);
    let setup = unset
        .post("/api/setup")
        .json(&serde_json::json!({ "token": token, "password": huge, "public_url": ORIGIN }))
        .send()
        .await
        .unwrap();
    assert_eq!(setup.status(), 413);
    assert!(!unset.state.operator.is_set_up().unwrap());

    let c = Collector::start(true).await;
    assert_eq!(c.login(&huge).await.status(), 413);
    assert_eq!(c.state.operator.verifications(), 0);
}

/// A logged request must never show the password.
#[test]
fn a_login_request_does_not_show_its_password_in_debug() {
    let shown = format!(
        "{:?}",
        LoginRequest {
            password: PASSWORD.into()
        }
    );
    assert!(!shown.contains(PASSWORD), "{shown}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --test login`
Expected: FAIL to compile. `error[E0432]: unresolved import hennery_proto::rest::LoginRequest`, and `error[E0425]: cannot find value MAX_BODY_BYTES in module hennery_kernel::auth_api`.

- [ ] **Step 3: The rules, the limiter and the endpoints**

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust
//!   still has its own separate budget; only addresses past capacity share
//!   the overflow entry's. There is no exemption for loopback.
//!
```

with:

```rust
//!   still has its own separate budget; only addresses past capacity share
//!   the overflow entry's.
//! - **Loopback is one address with its own entry (3b decision 9).** Every
//!   `127.0.0.0/8` address counts as `127.0.0.1` (on Linux the whole block
//!   reaches `lo`, so per-address entries would give a local process
//!   millions of budgets), and loopback always gets its own entry, past
//!   capacity too: `hennery up`'s own host and a reverse proxy on the same
//!   machine are never pushed into the shared overflow budget by a flood
//!   from elsewhere. It is not exempt: its budget is the same as anyone's.
//!
```

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;
```

with:

```rust
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;
```

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust
        window: Duration::from_secs(10 * 60),
        first_lockout: Duration::from_secs(60),
```

with:

```rust
        window: Duration::from_secs(10 * 60),
        first_lockout: Duration::from_secs(60),
        max_lockout: Duration::from_secs(60 * 60),
    };

    /// Operator login and step-up: 5 wrong passwords per minute, then
    /// exponential backoff (kernel spec §3.2).
    pub const LOGIN: Policy = Policy {
        free_failures: 5,
        window: Duration::from_secs(60),
        first_lockout: Duration::from_secs(60),
```

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust
/// What a client address is counted as: its IPv4 address (also when it
/// arrives IPv4-mapped), or its IPv6 /64.
pub fn key(addr: IpAddr) -> IpAddr {
    match addr.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
```

with:

```rust
/// What a client address is counted as: its IPv4 address (also when it
/// arrives IPv4-mapped), or its IPv6 /64. Every IPv4 loopback address is
/// `127.0.0.1`, and `::1` is itself.
pub fn key(addr: IpAddr) -> IpAddr {
    match addr.to_canonical() {
        IpAddr::V4(v4) if v4.is_loopback() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V4(v4) => IpAddr::V4(v4),
        // Not masked: its /64 would be `::`, which is not loopback.
        IpAddr::V6(v6) if v6.is_loopback() => IpAddr::V6(v6),
        IpAddr::V6(v6) => {
```

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust
        // A new address: forgotten entries are pruned to make room, but a
        // live one is never evicted for it (amendment 2026-10-01).
        state.entries.retain(|_, e| !e.forgotten(&policy, now));
        if state.entries.len() < self.capacity {
            let entry = state.entries.entry(addr).or_insert(Entry {
```

with:

```rust
        // A new address: forgotten entries are pruned to make room, but a
        // live one is never evicted for it (amendment 2026-10-01). Loopback
        // gets its own entry regardless (3b decision 9).
        state.entries.retain(|_, e| !e.forgotten(&policy, now));
        if state.entries.len() < self.capacity || addr.is_loopback() {
            let entry = state.entries.entry(addr).or_insert(Entry {
```

In `crates/hennery-kernel/src/ratelimit.rs`, replace:

```rust

    /// The all-locked case: every tracked address is already locked out
```

with:

```rust

    /// 3b decision 9 (M3 of 3a's final review): past capacity, loopback
    /// still gets its own entry, so a flood from elsewhere cannot use up
    /// `hennery up`'s own budget; and the whole of `127.0.0.0/8` is that
    /// one entry, so a local process cannot rotate through it.
    #[test]
    fn loopback_keeps_one_entry_of_its_own_past_capacity() {
        let limiter = Limiter::with_capacity(Policy::ENROLL, 1);
        let t0 = Instant::now();
        limiter.attempt(A, t0).unwrap();
        // The table is full of a live entry: a new address overflows.
        for _ in 0..5 {
            limiter.attempt(B, t0).unwrap();
        }
        assert!(limiter.attempt(B, t0).is_err(), "the overflow budget is used up");
        let lo = |d: u8| IpAddr::V4(std::net::Ipv4Addr::new(127, d, 0, 1));
        assert_eq!(key(lo(9)), IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        for d in 0..5 {
            assert_eq!(
                limiter.attempt(lo(d), t0),
                Ok(()),
                "loopback shared the overflow budget"
            );
        }
        assert!(limiter.attempt(lo(200), t0).is_err(), "127/8 is one address");
        assert_eq!(limiter.tracked(), 2);
        // `::1` is loopback too, with its own entry.
        assert_eq!(limiter.attempt(IpAddr::V6(Ipv6Addr::LOCALHOST), t0), Ok(()));
    }

    /// The all-locked case: every tracked address is already locked out
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust

use crate::secret::{random_bytes, sha256_hex};
```

with:

```rust

use crate::ratelimit::{Limiter, Policy};
use crate::secret::{random_bytes, sha256_hex};
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    verifications: AtomicU64,
}
```

with:

```rust
    verifications: AtomicU64,
    /// Wrong passwords per client address, at login and step-up (kernel
    /// spec §3.2).
    pub login_limiter: Limiter,
}
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            verifications: AtomicU64::new(0),
        })
```

with:

```rust
            verifications: AtomicU64::new(0),
            login_limiter: Limiter::new(Policy::LOGIN),
        })
```

Create `crates/hennery-kernel/src/origin.rs`:

```rust
//! The browser rules (kernel spec §3.3): what every browser route checks
//! before its handler, cookie or not.
//!
//! - **State-changing methods** (every method but `GET` and `HEAD`): `Origin`
//!   must be `public_url`'s, and a missing `Origin` is refused. Before setup
//!   there is no `public_url`, so they are refused outright. The body must be
//!   JSON: a `Content-Type` other than `application/json` is refused, and so
//!   is a body without one. A body-less request (park, logout, revoke) needs
//!   no `Content-Type`.
//! - **`GET` and `HEAD`**: browsers send no `Origin` on a same-origin `GET`,
//!   so `Sec-Fetch-Site` is checked instead: `same-origin` or `none`, or
//!   absent (a client that is not a browser, or one too old to send it;
//!   the `SameSite=Strict` cookie still keeps cross-site requests out). A
//!   present `Origin` must match.
//!
//! No `GET` or `HEAD` route changes state; one that must is a `POST`.
//! Accepting a missing `Sec-Fetch-Site` relies on this.
//!
//! Host enrollment, the host WebSocket and setup are not browser routes in
//! this sense and sit outside this layer (setup has its own rule).

use crate::auth_api::error;
use crate::operator::Operator;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

pub async fn browser_rules(State(operator): State<Arc<Operator>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    // A header that is not visible ASCII matches nothing.
    let origin = headers.get(header::ORIGIN).map(|v| v.to_str().unwrap_or("\u{0}"));
    let public_url = operator.public_url();
    let expected = public_url.as_ref().map(|u| u.origin());
    if is_state_changing(req.method()) {
        if expected.is_none() {
            return error(StatusCode::FORBIDDEN, "setup_required", "hennery is not set up yet");
        }
        if origin != expected {
            tracing::debug!(?expected, received = ?origin, "request refused: origin_mismatch");
            return error(
                StatusCode::FORBIDDEN,
                "origin_mismatch",
                "state-changing requests must come from the public_url's origin",
            );
        }
        if !is_json_or_empty(headers) {
            return error(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "the body must be application/json",
            );
        }
    } else {
        let site = headers.get("sec-fetch-site").map(|v| v.to_str().unwrap_or(""));
        if site.is_some_and(|s| s != "same-origin" && s != "none") {
            return error(StatusCode::FORBIDDEN, "cross_site", "cross-site requests are refused");
        }
        if origin.is_some() && origin != expected {
            tracing::debug!(?expected, received = ?origin, "request refused: origin_mismatch");
            return error(
                StatusCode::FORBIDDEN,
                "origin_mismatch",
                "a request's Origin must be the public_url's",
            );
        }
    }
    next.run(req).await
}

fn is_state_changing(method: &Method) -> bool {
    method != Method::GET && method != Method::HEAD
}

/// `application/json` (any parameters), or no body at all.
fn is_json_or_empty(headers: &HeaderMap) -> bool {
    match headers.get(header::CONTENT_TYPE) {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.split(';').next())
            .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json")),
        None => {
            !headers.contains_key(header::TRANSFER_ENCODING)
                && headers
                    .get(header::CONTENT_LENGTH)
                    .is_none_or(|len| len.to_str().is_ok_and(|len| len.trim() == "0"))
        }
    }
}
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup.

use crate::operator::{Operator, PublicUrl, SetupOutcome, session_cookie};
use crate::secret::unix_now;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
```

with:

```rust
//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup, and
//! login and logout.

use crate::operator::{Operator, PublicUrl, SetupOutcome, cleared_cookie, session_cookie, session_token};
use crate::secret::unix_now;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, SetupRequest, SetupResponse};
use std::sync::Arc;

/// The operator auth routes, each with its own `Origin` rule (kernel spec
/// §3.3).
pub fn router(operator: Arc<Operator>) -> Router {
    let private = || middleware::map_response(crate::setup_page::private_headers);
```

with:

```rust
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, LoginRequest, SetupRequest, SetupResponse};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The largest request body the auth routes read: a password is at most
/// 1024 bytes (`MAX_PASSWORD_BYTES`), so anything much larger is refused
/// with 413 before it is parsed.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// The operator auth routes. Setup has its own `Origin` rule; the rest are
/// browser routes (kernel spec §3.3). Serve with `ConnectInfo<SocketAddr>`:
/// login rate-limits on the peer address.
pub fn router(operator: Arc<Operator>) -> Router {
    let browser = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .layer(middleware::from_fn_with_state(
            operator.clone(),
            crate::origin::browser_rules,
        ));
    let private = || middleware::map_response(crate::setup_page::private_headers);
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
        .route("/setup.js", get(crate::setup_page::script).layer(private()))
        .with_state(operator)
```

with:

```rust
        .route("/setup.js", get(crate::setup_page::script).layer(private()))
        .merge(browser)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(operator)
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
        .to_string()
}
```

with:

```rust
        .to_string()
}

/// 429 with `Retry-After`, in whole seconds rounded up.
pub(crate) fn rate_limited(retry_after: Duration, message: &str) -> Response {
    let mut response = error(StatusCode::TOO_MANY_REQUESTS, "rate_limited", message);
    let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    response
}

/// Whether the session cookie is `Secure`: as `public_url` says, and
/// `Secure` when there is none yet.
pub(crate) fn secure_cookies(operator: &Operator) -> bool {
    operator.public_url().is_none_or(|u| u.is_https())
}
```

Append to `crates/hennery-kernel/src/auth_api.rs`:

```rust
/// `POST /api/auth/login`: 204 and a new session cookie. Every attempt
/// counts against the client's address until one succeeds; an attempt
/// that is not refused for that runs exactly one password check, right or
/// wrong, so the answer takes as long either way (kernel spec §3.2).
async fn login(
    State(operator): State<Arc<Operator>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Response {
    if let Err(retry_after) = operator.login_limiter.attempt(peer.ip(), Instant::now()) {
        return rate_limited(
            retry_after,
            "too many wrong passwords from this address; try again later",
        );
    }
    match operator.check_password(req.password).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    }
    operator.login_limiter.succeeded(peer.ip());
    match operator.open_session(&user_agent(&headers), unix_now()) {
        Ok(Some(token)) => with_cookie(
            StatusCode::NO_CONTENT.into_response(),
            &session_cookie(&token, secure_cookies(&operator)),
        ),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => internal(err),
    }
}

/// `POST /api/auth/logout`: 204, the request's session (if any) ended and
/// its cookie cleared.
async fn logout(State(operator): State<Arc<Operator>>, headers: HeaderMap) -> Response {
    if let Some(token) = session_token(&headers) {
        match operator.authenticate(token, unix_now()) {
            Ok(Some(session)) => {
                if let Err(err) = operator.revoke_session(&session.session_id) {
                    return internal(err);
                }
            }
            Ok(None) => {}
            Err(err) => return internal(err),
        }
    }
    with_cookie(
        StatusCode::NO_CONTENT.into_response(),
        &cleared_cookie(secure_cookies(&operator)),
    )
}
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
pub mod operator;
pub mod ratelimit;
```

with:

```rust
pub mod operator;
pub mod origin;
pub mod ratelimit;
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// `POST /api/auth/login` (kernel spec §3.2): the owner's password. 204 and
/// the session cookie on success. `Debug` leaves the password out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct LoginRequest {
    pub password: String,
}

impl std::fmt::Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest").finish_non_exhaustive()
    }
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SetupRequest,
        rest::SetupResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::SetupRequest,
        rest::SetupResponse,
        rest::LoginRequest,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SetupResponse,
    );
```

with:

```rust
        rest::SetupResponse,
        rest::LoginRequest,
    );
```

- [ ] **Step 4: Regenerate and run the new tests**

Run: `cargo run -p hennery-proto --bin gen`, then `cargo test -p hennery-testkit --test login --locked && cargo test -p hennery-kernel --lib ratelimit --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probe the limiter, loopback, the rules, the body limit and `Debug`**

Each probe is a one-line change. Rerun the named test, see it fail, then restore the code.
1. In `login`, change `if let Err(retry_after) = operator.login_limiter.attempt(peer.ip(), Instant::now())` to `if let (false, Err(retry_after)) = (true, operator.login_limiter.attempt(peer.ip(), Instant::now()))`. Expected: `wrong_passwords_lock_the_address_out_and_each_attempt_checks_once` fails.
2. In `Limiter::attempt`, remove ` || addr.is_loopback()`. Expected: `loopback_keeps_one_entry_of_its_own_past_capacity` fails.
3. In `browser_rules`, change `if origin != expected {` to `if false && origin != expected {`. Expected: `login_needs_the_public_urls_origin_json_and_setup` fails.
4. In `browser_rules`, change `if !is_json_or_empty(headers) {` to `if false && !is_json_or_empty(headers) {`. Expected: the same test fails.
5. In `browser_rules`, change `if expected.is_none() {` to `if false && expected.is_none() {`. Expected: the same test fails.
6. In `auth_api::router`, remove the line `.layer(DefaultBodyLimit::max(MAX_BODY_BYTES))`. Expected: `an_oversized_body_is_refused_before_it_is_parsed` fails.
7. In `LoginRequest`'s `Debug`, change `f.debug_struct("LoginRequest").finish_non_exhaustive()` to `f.debug_struct("LoginRequest").field("password", &self.password).finish()`. Expected: `a_login_request_does_not_show_its_password_in_debug` fails.

- [ ] **Step 6: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 424 tests pass.

- [ ] **Step 7: Commit and push**

```bash
git add crates schema web/src/generated
git commit -m "feat(kernel): login and logout, rate limited, behind the browser Origin rules"
git push
```

### Task 5: The session cookie replaces the development bearer on every operator route

**Files:**
- Replace: `crates/hennery-kernel/src/auth.rs` (`DevToken` and `require_bearer` go; `operator_only` and `require_operator` come)
- Modify: `crates/hennery-sessions/src/lib.rs` (no `token`), `crates/hennery-sessions/src/api.rs`, `crates/hennery-sessions/src/hosts.rs`, `crates/hennery/src/main.rs` (no `--dev-token`), `crates/hennery-testkit/Cargo.toml`, `crates/hennery-testkit/src/lib.rs`
- Modify (every harness on the cookie): `crates/hennery-testkit/tests/{e2e,reconcile,join,pairing,ws_ingest_error,setup,login}.rs`, `crates/hennery/tests/cli.rs`
- Test: `crates/hennery-testkit/tests/auth.rs` (the route table), `crates/hennery/tests/cli.rs`

**Interfaces:**
- Consumes: Tasks 1–4.
- Produces (`hennery_kernel::auth`):
  - `fn operator_only<S>(Router<S>, Arc<Operator>) -> Router<S>`: the browser rules outermost, then `require_operator`;
  - `async fn require_operator(State<Arc<Operator>>, Request, Next) -> Response`: 401 `unauthenticated`, else the `Authenticated` session goes into the request's extensions, and the cookie is re-sent when the request slid it.

  `DevToken`, `MIN_DEV_TOKEN_LEN` and `require_bearer` no longer exist.
- Produces (`hennery_sessions`): `AppState::new(store: Store, hosts: Hosts, operator: Operator)`. The field `token` is gone.
- Produces (`hennery_testkit`): consts `PUBLIC_URL = "https://hennery.example"` and `OWNER_PASSWORD`, and `fn operator_client(&Operator) -> reqwest::Client`. The client sets the collector up if it is not, opens a session, and sends `Cookie` and `Origin` on every request.
- Produces (binary): `collector` and `up` take no `--dev-token` (clap refuses it), and `up` sets no `HENNERY_DEV_TOKEN` for its collector.

- [ ] **Step 1: Write the failing tests**

`auth.rs` replaces the bearer test with a table of every operator route. Every other harness moves to the cookie:
- `client()` becomes `client(&collector)`;
- the testkit's harnesses call `hennery_testkit::operator_client`;
- the CLI tests sign in through `setup-url` (`sign_in`), and send `Cookie` (and `Origin` on state-changing requests).

The two short-token CLI tests go, and `the_development_token_flag_is_gone` takes their place (decision 11).

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
//! Auth negative paths: a protected REST route without (or with the wrong)
//! bearer token must be rejected, and a host `hello` without a valid proof
//! of its key must be rejected without ever registering the host (ACP core
//! §3.5, kernel spec §11). `axum`'s `.layer()` only wraps routes added
//! *before* it in the router builder, so a route added after would silently
//! escape auth — this pins that every REST route is actually covered.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::rest::HostItem;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
```

with:

```rust
//! Auth negative paths: an operator route without (or with the wrong)
//! session cookie, or from another origin, must be rejected, and a host
//! `hello` without a valid proof of its key must be rejected without ever
//! registering the host (ACP core §3.5, kernel spec §3.3, §11). `axum`'s
//! `.layer()` only wraps routes added *before* it in the router builder, so
//! a route added after would silently escape auth — the route table below
//! pins that every operator route is actually covered.

use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::Operator;
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::rest::{ApiError, HostItem};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust

const TOKEN: &str = "dev-token-for-tests";
const HOST: &str = "host-1";
```

with:

```rust

const HOST: &str = "host-1";
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
```

with:

```rust
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
        );
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust

#[tokio::test]
async fn a_request_without_or_with_the_wrong_bearer_token_is_rejected() {
    let collector = Collector::start().await;
    let plain = reqwest::Client::new();

    let no_auth = plain.get(collector.url("/api/hosts")).send().await.unwrap();
    assert_eq!(no_auth.status(), 401);

    let wrong = plain
        .get(collector.url("/api/hosts"))
        .bearer_auth("not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    // Sanity: the right token still gets through, proving the 401s above are
    // about the credential and not a broken test harness.
    let right = plain
        .get(collector.url("/api/hosts"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(right.status(), 200);
    let hosts: Vec<HostItem> = right.json().await.unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!((hosts[0].host_id.as_str(), hosts[0].connected), (HOST, false));

    collector.stop().await;
```

with:

```rust

/// Every operator route, as a method and a path. A route missing from
/// here is a route nobody checked.
const OPERATOR_ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/hosts"),
    ("POST", "/api/hosts/pairing-codes"),
    ("DELETE", "/api/hosts/host-9"),
    ("POST", "/api/sessions"),
    ("GET", "/api/sessions/s-1"),
    ("POST", "/api/sessions/s-1/resume"),
    ("POST", "/api/sessions/s-1/prompt"),
    ("POST", "/api/sessions/s-1/cancel"),
    ("POST", "/api/sessions/s-1/park"),
    ("POST", "/api/sessions/s-1/close"),
    ("GET", "/api/sessions/s-1/catalog"),
    ("POST", "/api/sessions/s-1/config"),
    ("POST", "/api/sessions/s-1/pending/p-1/answer"),
    ("GET", "/api/sessions/s-1/events"),
    ("GET", "/api/stream/sessions/s-1"),
];

/// Bounded, so a route that escaped the layer and streams (SSE) fails the
/// test instead of hanging it.
fn request(client: &reqwest::Client, collector: &Collector, method: &str, path: &str) -> reqwest::RequestBuilder {
    client
        .request(method.parse().unwrap(), collector.url(path))
        .timeout(std::time::Duration::from_secs(10))
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (
        status,
        resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
    )
}

#[tokio::test]
async fn every_operator_route_needs_the_session_cookie() {
    let collector = Collector::start().await;
    let signed_in = hennery_testkit::operator_client(&collector.state.operator);
    let plain = reqwest::Client::new();
    for &(method, path) in OPERATOR_ROUTES {
        let none = request(&plain, &collector, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(none).await, (401, "unauthenticated".into()), "{method} {path}");
        let wrong = request(&plain, &collector, method, path)
            .header("origin", hennery_testkit::PUBLIC_URL)
            .header("cookie", format!("hennery_session={}", "0".repeat(64)))
            .send()
            .await
            .unwrap();
        assert_eq!(code_of(wrong).await, (401, "unauthenticated".into()), "{method} {path}");
        // Sanity: the owner's session gets through, proving the 401s above
        // are about the cookie and not a broken harness.
        let status = request(&signed_in, &collector, method, path)
            .send()
            .await
            .unwrap()
            .status();
        assert!(status != 401 && status != 403, "{method} {path}: {status}");
    }
    let hosts: Vec<HostItem> = signed_in
        .get(collector.url("/api/hosts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!((hosts[0].host_id.as_str(), hosts[0].connected), (HOST, false));
    collector.stop().await;
}

/// Kernel spec §3.3, with a valid session: state-changing requests need the
/// `public_url`'s `Origin`; `GET`s are refused when the browser says they
/// are cross-site, or carry another `Origin`.
#[tokio::test]
async fn every_operator_route_applies_the_browser_rules() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let token = collector
        .state
        .operator
        .open_session("test", hennery_kernel::secret::unix_now())
        .unwrap()
        .unwrap();
    let cookie = format!("hennery_session={token}");
    let plain = reqwest::Client::new();
    for &(method, path) in OPERATOR_ROUTES {
        let send = |origin: Option<&str>, site: Option<&str>| {
            let mut req = request(&plain, &collector, method, path).header("cookie", &cookie);
            if let Some(origin) = origin {
                req = req.header("origin", origin);
            }
            if let Some(site) = site {
                req = req.header("sec-fetch-site", site);
            }
            req.send()
        };
        let evil = code_of(send(Some("https://evil.example"), None).await.unwrap()).await;
        assert_eq!(evil, (403, "origin_mismatch".into()), "{method} {path}");
        if method == "GET" {
            for site in ["cross-site", "same-site"] {
                let resp = send(None, Some(site)).await.unwrap();
                assert_eq!(
                    code_of(resp).await,
                    (403, "cross_site".into()),
                    "{method} {path} {site}"
                );
            }
            for site in [Some("same-origin"), Some("none"), None] {
                let status = send(None, site).await.unwrap().status();
                assert!(status != 401 && status != 403, "{method} {path} {site:?}: {status}");
            }
        } else {
            let missing = code_of(send(None, None).await.unwrap()).await;
            assert_eq!(missing, (403, "origin_mismatch".into()), "{method} {path}");
        }
    }
    collector.stop().await;
}

/// Enrollment and the host WebSocket are authenticated otherwise and are
/// exempt from the browser rules (kernel spec §3.3): no cookie, any origin.
#[tokio::test]
async fn enrollment_and_the_host_socket_need_neither_a_session_nor_an_origin() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let enroll = reqwest::Client::new()
        .post(collector.url("/api/hosts/enroll"))
        .header("origin", "https://evil.example")
        .json(&serde_json::json!({
            "code": "0000-0000", "public_key": host_key().public_key_hex(),
            "name": "x", "host_version": "x", "platform": "x"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(enroll).await, (401, "invalid_code".into()));
    let (mut ws, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert!(matches!(
        hello(&mut ws, HOST, proof).await,
        CollectorFrame::HelloAck { .. }
    ));
    collector.stop().await;
}

/// A request that slides the session's expiry sends the cookie again, so
/// the browser's copy is extended with it; one that does not, sends none.
#[tokio::test]
async fn a_request_that_slides_the_session_sends_its_cookie_again() {
    let collector = Collector::start().await;
    hennery_testkit::operator_client(&collector.state.operator);
    let now = hennery_kernel::secret::unix_now();
    let url = collector.url("/api/hosts");
    let fresh = collector.state.operator.open_session("test", now).unwrap().unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={fresh}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("set-cookie").is_none());

    let stale = collector
        .state
        .operator
        .open_session("test", now - 120)
        .unwrap()
        .unwrap();
    let resp = reqwest::Client::new()
        .get(&url)
        .header("cookie", format!("hennery_session={stale}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let cookie = resp.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with(&format!("hennery_session={stale};")), "{cookie}");
    collector.stop().await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{Enrollment, Hosts};
```

with:

```rust
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::hosts::{Enrollment, Hosts};
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
use std::time::Duration;

const TOKEN: &str = "dev-token-for-tests";

```

with:

```rust
use std::time::Duration;

```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        pair_host(&hosts);
        let mut state = AppState::new(
            Store::open(db).unwrap(),
            hosts,
            Operator::open(db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
        state.offline_threshold = offline;
```

with:

```rust
        pair_host(&hosts);
        let mut state = AppState::new(Store::open(db).unwrap(), hosts, Operator::open(db).unwrap());
        state.offline_threshold = offline;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust

fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}
```

with:

```rust

fn client(collector: &Collector) -> reqwest::Client {
    hennery_testkit::operator_client(&collector.state.operator)
}
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client();
    wait_host_connected(&c, &collector).await;

    let session = start_session(&c, &collector).await;
```

with:

```rust
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;

    let session = start_session(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &slow);
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &slow);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    c.post(collector.url(&format!("/api/sessions/{session}/prompt")))
```

with:

```rust
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    c.post(collector.url(&format!("/api/sessions/{session}/prompt")))
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client();
    wait_host_connected(&c, &collector).await;

```

with:

```rust
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;

```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let c = client();

```

with:

```rust
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let c = client(&collector);

```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
```

with:

```rust
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let prompt_url = collector.url(&format!("/api/sessions/{session}/prompt"));
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        ..slow_script(20)
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let grandchild = wait_for("grandchild pid", || async { pid_from(&pid_file) }).await;
```

with:

```rust
        ..slow_script(20)
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let grandchild = wait_for("grandchild pid", || async { pid_from(&pid_file) }).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &FakeScript::default());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let host = start_host_with(collector.addr, &dir.path().join("host"), counting.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    let host = start_host_with(collector.addr, &dir.path().join("host"), counting.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &slow_script(6));
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &slow_script(6));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    }));
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    }));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &history_and_state());
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &history_and_state());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    post_json(&c, collector.url(&format!("/api/sessions/{session}/park")), json!({})).await;
```

with:

```rust
        ..FakeScript::default()
    };
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    post_json(&c, collector.url(&format!("/api/sessions/{session}/park")), json!({})).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    ));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
```

with:

```rust
    ));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &slow_script(20));
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &slow_script(20));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client();
    wait_host_connected(&c, &collector).await;
    let (status, body) = post_json(
```

with:

```rust
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let (status, body) = post_json(
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &config_script(&log));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt_and_wait(&c, &collector, &session, 1).await;
```

with:

```rust
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    prompt_and_wait(&c, &collector, &session, 1).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let script = asking(vec![FakeAsk::Permission, FakeAsk::Elicitation]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
```

with:

```rust
    let script = asking(vec![FakeAsk::Permission, FakeAsk::Elicitation]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let script = asking(vec![hennery_testkit::FakeAsk::Permission]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
```

with:

```rust
    let script = asking(vec![hennery_testkit::FakeAsk::Permission]);
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    start_host(collector.addr, &dir.path().join("host"), &script);
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    let host = start_host_with(collector.addr, &dir.path().join("host"), fake.clone());
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    );
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    );
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
    )));
    let c = client();
    wait_host_connected(&c, &collector).await;
```

with:

```rust
    )));
    let c = client(&collector);
    wait_host_connected(&c, &collector).await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use hennery_host::identity::HostKey;
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{Enrollment, Hosts};
```

with:

```rust
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust

const TOKEN: &str = "dev-token-for-tests";
const HOST: &str = "host-1";
```

with:

```rust

const HOST: &str = "host-1";
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
```

with:

```rust
            Operator::open(&dir.path().join("hennery.db")).unwrap(),
        );
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust

fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}
```

with:

```rust

fn client(collector: &Collector) -> reqwest::Client {
    hennery_testkit::operator_client(&collector.state.operator)
}
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
async fn started_session(collector: &Collector, host: &mut ScriptedHost) -> String {
    let c = client();
    let url = collector.url("/api/sessions");
```

with:

```rust
async fn started_session(collector: &Collector, host: &mut ScriptedHost) -> String {
    let c = client(collector);
    let url = collector.url("/api/sessions");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, body) = post(
        &client(),
        collector.url("/api/sessions"),
```

with:

```rust
    let (status, body) = post(
        &client(&collector),
        collector.url("/api/sessions"),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client();
    let url = collector.url("/api/sessions");
    let call =
```

with:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client(&collector);
    let url = collector.url("/api/sessions");
    let call =
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust

    let c = client();
    let url = prompt_url.clone();
```

with:

```rust

    let c = client(&collector);
    let url = prompt_url.clone();
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // Still wedged until the host is back: one turn at a time.
    assert_eq!(post(&client(), prompt_url.clone(), prompt_body()).await.0, 409);

```

with:

```rust
    // Still wedged until the host is back: one turn at a time.
    assert_eq!(
        post(&client(&collector), prompt_url.clone(), prompt_body()).await.0,
        409
    );

```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    );
    let c = client();
    let url = prompt_url.clone();
```

with:

```rust
    );
    let c = client(&collector);
    let url = prompt_url.clone();
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = started_session(&collector, &mut host).await;
    let c = client();
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
```

with:

```rust
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        "{kinds:?}"
    );
    let (status, body) = post(
        &client(),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    )
```

with:

```rust
        "{kinds:?}"
    );
    let (status, body) = post(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    )
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // `Hub::request_for_session` — no waiter is registered for it).
    let c = client();
    let url = collector.url(&format!("/api/sessions/{session}/close"));
```

with:

```rust
    // `Hub::request_for_session` — no waiter is registered for it).
    let c = client(&collector);
    let url = collector.url(&format!("/api/sessions/{session}/close"));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = started_session(&collector, &mut host).await;
    let c = client();
    let url = collector.url(&format!("/api/sessions/{session}/close"));
```

with:

```rust
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = collector.url(&format!("/api/sessions/{session}/close"));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust

    let c = client();
    let url = resume_url(&collector, &session);
```

with:

```rust

    let c = client(&collector);
    let url = resume_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
async fn started_turn(collector: &Collector, host: &mut ScriptedHost, session: &str) -> String {
    let c = client();
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
```

with:

```rust
async fn started_turn(collector: &Collector, host: &mut ScriptedHost, session: &str) -> String {
    let c = client(collector);
    let url = collector.url(&format!("/api/sessions/{session}/prompt"));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, body) = post(
        &client(),
        collector.url(&format!("/api/sessions/{session}/prompt")),
```

with:

```rust
    let (status, body) = post(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/prompt")),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
async fn a_resume_attaches_a_parked_session_again() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client();
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    assert_eq!(collector.lifecycle(&session), "starting");
```

with:

```rust
async fn a_resume_attaches_a_parked_session_again() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    assert_eq!(collector.lifecycle(&session), "starting");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = parked_session(&collector, &mut host).await;
    let c = client();
    let url = resume_url(&collector, &session);
    let first = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    let (status, body) = post(&client(), resume_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("starting")), "{body}");
```

with:

```rust
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let first = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
    let (status, body) = post(&client(&collector), resume_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("starting")), "{body}");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client();
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
```

with:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_resume(&mut host, &session).await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // Failed is resumable: the operator may try again.
    let c = client();
    let url = resume_url(&collector, &session);
```

with:

```rust
    // Failed is resumable: the operator may try again.
    let c = client(&collector);
    let url = resume_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    for code in ["not_attached", "invalid"] {
        let c = client();
        let url = resume_url(&collector, &session);
```

with:

```rust
    for code in ["not_attached", "invalid"] {
        let c = client(&collector);
        let url = resume_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let active = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(), resume_url(&collector, &active), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("active")));

    // A start the host failed: the agent never created a session.
    let c = client();
    let url = collector.url("/api/sessions");
```

with:

```rust
    let active = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(&collector), resume_url(&collector, &active), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("active")));

    // A start the host failed: the agent never created a session.
    let c = client(&collector);
    let url = collector.url("/api/sessions");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    assert_eq!(call.await.unwrap().0, 502);
    let (status, body) = post(&client(), resume_url(&collector, &session_id), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("agent_has_no_record")));
```

with:

```rust
    assert_eq!(call.await.unwrap().0, 502);
    let (status, body) = post(&client(&collector), resume_url(&collector, &session_id), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("agent_has_no_record")));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    host.drop_connection(&collector).await;
    let (status, body) = post(&client(), resume_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
```

with:

```rust
    host.drop_connection(&collector).await;
    let (status, body) = post(&client(&collector), resume_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    );
    let (status, _) = post(&client(), resume_url(&collector, "no-such-session"), json!({})).await;
    assert_eq!(status, 404);
```

with:

```rust
    );
    let (status, _) = post(
        &client(&collector),
        resume_url(&collector, "no-such-session"),
        json!({}),
    )
    .await;
    assert_eq!(status, 404);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // never sent, so it cannot race the resend (ACP core §5.1).
    let (status, body) = post(&client(), resume_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
```

with:

```rust
    // never sent, so it cannot race the resend (ACP core §5.1).
    let (status, body) = post(&client(&collector), resume_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("host_offline")));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = parked_session(&collector, &mut host).await;
    let c = client();
    let url = resume_url(&collector, &session);
```

with:

```rust
    let session = parked_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = resume_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let turn = started_turn(&collector, &mut host, &session).await;
    let (status, body) = get(&client(), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{body}");
```

with:

```rust
    let turn = started_turn(&collector, &mut host, &session).await;
    let (status, body) = get(&client(&collector), collector.url(&format!("/api/sessions/{session}"))).await;
    assert_eq!(status, 200, "{body}");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    );
    let (status, _) = get(&client(), collector.url("/api/sessions/no-such-session")).await;
    assert_eq!(status, 404);
```

with:

```rust
    );
    let (status, _) = get(&client(&collector), collector.url("/api/sessions/no-such-session")).await;
    assert_eq!(status, 404);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
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
```

with:

```rust
/// POST with a client that gives up after 300 ms; resolves once it has.
fn post_and_give_up(c: reqwest::Client, url: String, body: Value) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let sent = c.post(url).json(&body).timeout(Duration::from_millis(300)).send().await;
        assert!(sent.is_err(), "the collector answered before the host did: {sent:?}");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let caller = post_and_give_up(
        collector.url("/api/sessions"),
```

with:

```rust
    let caller = post_and_give_up(
        client(&collector),
        collector.url("/api/sessions"),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = parked_session(&collector, &mut host).await;
    let caller = post_and_give_up(resume_url(&collector, &session), json!({}));
    let request_id = expect_resume(&mut host, &session).await;
```

with:

```rust
    let session = parked_session(&collector, &mut host).await;
    let caller = post_and_give_up(client(&collector), resume_url(&collector, &session), json!({}));
    let request_id = expect_resume(&mut host, &session).await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = started_session(&collector, &mut host).await;
    let caller = post_and_give_up(collector.url(&format!("/api/sessions/{session}/prompt")), prompt_body());
    let CollectorFrame::Prompt { request_id, .. } = host.next().await else {
```

with:

```rust
    let session = started_session(&collector, &mut host).await;
    let caller = post_and_give_up(
        client(&collector),
        collector.url(&format!("/api/sessions/{session}/prompt")),
        prompt_body(),
    );
    let CollectorFrame::Prompt { request_id, .. } = host.next().await else {
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    expect_cancel(&mut host, &session, &turn).await;
```

with:

```rust
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    expect_cancel(&mut host, &session, &turn).await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.emit(
```

with:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
    let request_id = expect_cancel(&mut host, &session, &turn).await;
    host.emit(
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(), cancel_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    let (status, _) = post(&client(), cancel_url(&collector, "no-such-session"), json!({})).await;
    assert_eq!(status, 404);
```

with:

```rust
    let session = started_session(&collector, &mut host).await;
    let (status, body) = post(&client(&collector), cancel_url(&collector, &session), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("no_open_turn")));
    let (status, _) = post(
        &client(&collector),
        cancel_url(&collector, "no-such-session"),
        json!({}),
    )
    .await;
    assert_eq!(status, 404);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // The host has no such turn in flight.
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
```

with:

```rust
    // The host has no such turn in flight.
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({})).await });
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let parked = parked_session(&collector, &mut host).await;
    let (status, body) = post(&client(), cancel_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
```

with:

```rust
    let parked = parked_session(&collector, &mut host).await;
    let (status, body) = post(&client(&collector), cancel_url(&collector, &parked), json!({})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_attached")));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let park_url = collector.url(&format!("/api/sessions/{session}/park"));
    let (status, body) = post(&client(), park_url.clone(), json!({})).await;
    assert_eq!(
```

with:

```rust
    let park_url = collector.url(&format!("/api/sessions/{session}/park"));
    let (status, body) = post(&client(&collector), park_url.clone(), json!({})).await;
    assert_eq!(
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let c = client();
    let call = tokio::spawn(async move { post(&c, park_url, json!({})).await });
```

with:

```rust
    let mut host = ScriptedHost::connect(&collector, vec![attached(&session, seq)], seq).await;
    let c = client(&collector);
    let call = tokio::spawn(async move { post(&c, park_url, json!({})).await });
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client();
    let url = cancel_url(&collector, &session);
```

with:

```rust
    let turn = started_turn(&collector, &mut host, &session).await;
    let c = client(&collector);
    let url = cancel_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client();
    let url = collector.url("/api/sessions");
```

with:

```rust
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let c = client(&collector);
    let url = collector.url("/api/sessions");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    .await;
    let c = client();
    let url = resume_url(&collector, &session_id);
```

with:

```rust
    .await;
    let c = client(&collector);
    let url = resume_url(&collector, &session_id);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client();
    let url = config_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "mode", "value": "plan" })).await });
    let (request_id, config_id, value) = expect_set_config(&mut host, &session).await;
```

with:

```rust
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = config_url(&collector, &session);
    let call = tokio::spawn(async move { post(&c, url, json!({ "config_id": "mode", "value": "plan" })).await });
    let (request_id, config_id, value) = expect_set_config(&mut host, &session).await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    assert_eq!(body["config_options"], json!([{"id": "mode", "currentValue": "plan"}]));
    let (status, catalog) = get(&client(), collector.url(&format!("/api/sessions/{session}/catalog"))).await;
    assert_eq!((status, catalog), (200, body));
```

with:

```rust
    assert_eq!(body["config_options"], json!([{"id": "mode", "currentValue": "plan"}]));
    let (status, catalog) = get(
        &client(&collector),
        collector.url(&format!("/api/sessions/{session}/catalog")),
    )
    .await;
    assert_eq!((status, catalog), (200, body));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    for (code, status) in [("unknown_option", 409), ("config_failed", 502), ("invalid", 400)] {
        let c = client();
        let url = config_url(&collector, &session);
```

with:

```rust
    for (code, status) in [("unknown_option", 409), ("config_failed", 502), ("invalid", 400)] {
        let c = client(&collector);
        let url = config_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, _) = post(
        &client(),
        config_url(&collector, &session),
```

with:

```rust
    let (status, _) = post(
        &client(&collector),
        config_url(&collector, &session),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, _) = post(
        &client(),
        config_url(&collector, "nope"),
```

with:

```rust
    let (status, _) = post(
        &client(&collector),
        config_url(&collector, "nope"),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, body) = post(
        &client(),
        config_url(&collector, &session),
```

with:

```rust
    let (status, body) = post(
        &client(&collector),
        config_url(&collector, &session),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let session = started_session(&collector, &mut host).await;
    let c = client();
    let url = config_url(&collector, &session);
```

with:

```rust
    let session = started_session(&collector, &mut host).await;
    let c = client(&collector);
    let url = config_url(&collector, &session);
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let url = collector.url(&format!("/api/sessions/{session}/catalog"));
    let (_, before) = get(&client(), url.clone()).await;
    assert_ne!(before["mode"], "plan", "{before}");
```

with:

```rust
    let url = collector.url(&format!("/api/sessions/{session}/catalog"));
    let (_, before) = get(&client(&collector), url.clone()).await;
    assert_ne!(before["mode"], "plan", "{before}");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    wait_for("the late read-back stored", || async {
        let (status, catalog) = get(&client(), url.clone()).await;
        (status == 200 && catalog["mode"] == "plan").then_some(())
```

with:

```rust
    wait_for("the late read-back stored", || async {
        let (status, catalog) = get(&client(&collector), url.clone()).await;
        (status == 200 && catalog["mode"] == "plan").then_some(())
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    use futures::StreamExt;
    let resp = client()
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
```

with:

```rust
    use futures::StreamExt;
    let resp = client(collector)
        .get(collector.url(&format!("/api/stream/sessions/{session}")))
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let detail_url = collector.url(&format!("/api/sessions/{session}"));
    let (_, detail) = get(&client(), detail_url.clone()).await;
    assert_eq!(detail["activity"], "blocked");
```

with:

```rust
    let detail_url = collector.url(&format!("/api/sessions/{session}"));
    let (_, detail) = get(&client(&collector), detail_url.clone()).await;
    assert_eq!(detail["activity"], "blocked");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
```

with:

```rust
    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(&collector), url.clone(), json!({"option_id": "allow"})).await;
    assert_eq!(status, 202, "{body}");
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // One answer per question, even before its verdict.
    let (status, body) = post(&client(), url.clone(), json!({"option_id": "reject"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("already_answered")));
```

with:

```rust
    // One answer per question, even before its verdict.
    let (status, body) = post(&client(&collector), url.clone(), json!({"option_id": "reject"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("already_answered")));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    .await;
    let (_, detail) = get(&client(), detail_url).await;
    assert_eq!(
```

with:

```rust
    .await;
    let (_, detail) = get(&client(&collector), detail_url).await;
    assert_eq!(
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    );
    let (status, body) = post(&client(), url, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
```

with:

```rust
    );
    let (status, body) = post(&client(&collector), url, json!({"option_id": "allow"})).await;
    assert_eq!((status, body["code"].as_str()), (409, Some("not_open")));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
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
```

with:

```rust
    let url = answer_url(&collector, &session, "p1");
    let (status, body) = post(&client(&collector), url.clone(), json!({"option_id": "maybe"})).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("invalid")), "{body}");
    let (status, _) = post(&client(&collector), url.clone(), json!({"action": "accept"})).await;
    assert_eq!(status, 400, "an elicitation's answer to a permission request");
    let (status, _) = post(&client(&collector), url, json!({"action": "whatever"})).await;
    assert_eq!(status, 422);
    let (status, body) = post(
        &client(&collector),
        answer_url(&collector, &session, "no-such-question"),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, body) = post(
        &client(),
        answer_url(&collector, &session, "p1"),
```

with:

```rust
    let (status, body) = post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
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
```

with:

```rust
async fn a_refused_answer_gets_its_verdict_from_its_questions_resolution() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    let request_id = expect_answer(&mut host, &session, "allow").await;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    host.drop_connection(&collector).await;
    post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
```

with:

```rust
    host.drop_connection(&collector).await;
    post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    post(
        &client(),
        answer_url(&collector, &session, "p1"),
```

with:

```rust
    post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
async fn revoke(collector: &Collector, host_id: &str) -> (u16, Value) {
    let resp = client()
        .delete(collector.url(&format!("/api/hosts/{host_id}")))
```

with:

```rust
async fn revoke(collector: &Collector, host_id: &str) -> (u16, Value) {
    let resp = client(collector)
        .delete(collector.url(&format!("/api/hosts/{host_id}")))
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    // An answer the host has, with no verdict yet.
    let (status, _) = post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
```

with:

```rust
    // An answer the host has, with no verdict yet.
    let (status, _) = post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (status, _) = post(
        &client(),
        answer_url(&collector, &session, "p1"),
```

with:

```rust
    let (status, _) = post(
        &client(&collector),
        answer_url(&collector, &session, "p1"),
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
use hennery_host::pairing::{Joined, join};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HelloCheck, Hosts};
```

with:

```rust
use hennery_host::pairing::{Joined, join};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HelloCheck, Hosts};
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
use std::net::SocketAddr;

const TOKEN: &str = "dev-token-for-tests";

```

with:

```rust
use std::net::SocketAddr;

```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
            Operator::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
```

with:

```rust
            Operator::open(&db).unwrap(),
        );
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
    async fn mint(&self) -> String {
        let resp = reqwest::Client::new()
            .post(format!("{}/api/hosts/pairing-codes", self.public_url()))
            .bearer_auth(TOKEN)
            .send()
```

with:

```rust
    async fn mint(&self) -> String {
        let resp = hennery_testkit::operator_client(&self.state.operator)
            .post(format!("{}/api/hosts/pairing-codes", self.public_url()))
            .send()
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
```

with:

```rust

use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust

const TOKEN: &str = "dev-token-for-tests";
/// Valid Ed25519 public keys (RFC 8032 §7.1, tests 1 to 3).
```

with:

```rust

/// Valid Ed25519 public keys (RFC 8032 §7.1, tests 1 to 3).
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust
            Operator::open(&db).unwrap(),
            DevToken::new(TOKEN).unwrap(),
        );
```

with:

```rust
            Operator::open(&db).unwrap(),
        );
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust
    async fn mint(&self) -> String {
        let resp = reqwest::Client::new()
            .post(self.url("/api/hosts/pairing-codes"))
            .bearer_auth(TOKEN)
            .send()
```

with:

```rust
    async fn mint(&self) -> String {
        let resp = hennery_testkit::operator_client(&self.state.operator)
            .post(self.url("/api/hosts/pairing-codes"))
            .send()
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust
#[tokio::test]
async fn a_minted_code_pairs_a_host_without_the_operator_bearer_once() {
    let collector = Collector::start().await;
```

with:

```rust
#[tokio::test]
async fn a_minted_code_pairs_a_host_without_the_operators_session_once() {
    let collector = Collector::start().await;
```

In `crates/hennery-testkit/tests/pairing.rs`, replace:

```rust
    let collector = Collector::start().await;
    let mint = reqwest::Client::new()
        .post(collector.url("/api/hosts/pairing-codes"))
        .send()
        .await
        .unwrap();
    assert_eq!(mint.status(), 401);
    // Enrollment is reached without a bearer: the 401 is the code's, with
    // an API error body, not the bearer layer's.
    assert_eq!(
```

with:

```rust
    let collector = Collector::start().await;
    // Set up, so the browser rules let the request through to the session
    // check; an `Origin` but no session cookie.
    hennery_testkit::operator_client(&collector.state.operator);
    let mint = reqwest::Client::new()
        .post(collector.url("/api/hosts/pairing-codes"))
        .header("origin", hennery_testkit::PUBLIC_URL)
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(mint).await, (401, "unauthenticated".into()));
    // Enrollment is reached without a session or an `Origin`: the 401 is
    // the code's, not the session layer's.
    assert_eq!(
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use hennery_host::identity::HostKey;
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{Enrollment, Hosts};
```

with:

```rust
use hennery_host::identity::HostKey;
use hennery_kernel::hosts::{Enrollment, Hosts};
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token-for-tests";

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

```

with:

```rust
use tokio_tungstenite::tungstenite::Message;

```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(
        store,
        paired_hosts(),
        Operator::open_in_memory().unwrap(),
        DevToken::new(TOKEN).unwrap(),
    );
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
```

with:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, paired_hosts(), Operator::open_in_memory().unwrap());
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust

    let state = AppState::new(
        store,
        paired_hosts(),
        Operator::open_in_memory().unwrap(),
        DevToken::new(TOKEN).unwrap(),
    );
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(hennery_sessions::serve(listener, state));
```

with:

```rust

    let state = AppState::new(store, paired_hosts(), Operator::open_in_memory().unwrap());
    let shutdown = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = hennery_testkit::operator_client(&state.operator);
    let server = tokio::spawn(hennery_sessions::serve(listener, state));
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    // it via `/api/hosts` rather than racing the HTTP call below against it.
    let client = reqwest::Client::new();
    let hosts_url = format!("http://{addr}/api/hosts");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let hosts: Vec<HostItem> = client
            .get(&hosts_url)
            .header("authorization", bearer())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if hosts.iter().any(|h| h.host_id == "host-1" && h.connected) {
```

with:

```rust
    // it via `/api/hosts` rather than racing the HTTP call below against it.
    let hosts_url = format!("http://{addr}/api/hosts");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let hosts: Vec<HostItem> = client.get(&hosts_url).send().await.unwrap().json().await.unwrap();
        if hosts.iter().any(|h| h.host_id == "host-1" && h.connected) {
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
                .post(url)
                .header("authorization", bearer())
                .json(&json!({ "host_id": "host-1", "agent": "fake", "cwd": "/tmp" }))
```

with:

```rust
                .post(url)
                .json(&json!({ "host_id": "host-1", "agent": "fake", "cwd": "/tmp" }))
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
        .get(format!("http://{addr}/api/sessions/{session_id}"))
        .header("authorization", bearer())
        .send()
```

with:

```rust
        .get(format!("http://{addr}/api/sessions/{session_id}"))
        .send()
```

In `crates/hennery-testkit/tests/setup.rs`, replace:

```rust

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

with:

```rust

use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/setup.rs`, replace:

```rust
            Operator::open_in_memory().unwrap(),
            DevToken::new("dev-token-for-tests").unwrap(),
        );
```

with:

```rust
            Operator::open_in_memory().unwrap(),
        );
```

In `crates/hennery-testkit/tests/login.rs`, replace:

```rust

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

with:

```rust

use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/login.rs`, replace:

```rust
            Operator::open_in_memory().unwrap(),
            DevToken::new("dev-token-for-tests").unwrap(),
        );
```

with:

```rust
            Operator::open_in_memory().unwrap(),
        );
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
        let token = hennery_kernel::auth::DevToken::new("dev-token-for-tests").unwrap();
        let operator = hennery_kernel::operator::Operator::open(&db).unwrap();
        let state = hennery_sessions::AppState::new(store, hosts, operator, token);
        let code = state
```

with:

```rust
        let hosts = hennery_kernel::hosts::Hosts::open(&db).unwrap();
        let operator = hennery_kernel::operator::Operator::open(&db).unwrap();
        let state = hennery_sessions::AppState::new(store, hosts, operator);
        let code = state
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

#[test]
fn a_short_dev_token_is_refused_at_start() {
    let dir = std::env::temp_dir().join(format!("hennery-cli-short-token-{}", std::process::id()));
    for command in ["collector", "up"] {
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args([command, "--listen", "127.0.0.1:0", "--dev-token", "short"])
            .arg("--data-dir")
            .arg(&dir)
            .output()
            .unwrap();
        assert!(!out.status.success(), "{command} started with a short token");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("at least 16 characters"), "{command}: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `collector` must reject a short dev token before it does anything to the
/// data directory: `run_up` already validates first (checked above), and
/// `run_collector` must too, the same way — not only after opening the
/// store and the host registry there.
#[test]
fn a_short_dev_token_stops_the_collector_before_it_touches_the_data_dir() {
    let dir = std::env::temp_dir().join(format!("hennery-cli-token-first-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["collector", "--listen", "127.0.0.1:0", "--dev-token", "short"])
        .arg("--data-dir")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!dir.exists(), "the data dir was created before the token was validated");
}
```

with:

```rust

/// Plan 3b: the development bearer is gone. `--dev-token` is refused, not
/// ignored, so a service still configured with it fails loudly.
#[test]
fn the_development_token_flag_is_gone() {
    for command in ["collector", "up"] {
        let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args([command, "--dev-token", "dev-token-for-tests"])
            .args(["--data-dir", "/nonexistent/hennery-cli-dev-token"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{command} took --dev-token");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("unexpected argument '--dev-token'"),
            "{command}: {stderr}"
        );
    }
}
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command
        .args(["up", "--listen", &listen])
        .arg("--data-dir")
        .arg(&dir)
        .args(["--dev-token", "dev-token-for-tests"]);
    // SAFETY: setpgid(0, 0) in the child, right after fork and before exec,
```

with:

```rust
    let mut command = Command::new(env!("CARGO_BIN_EXE_hennery"));
    command.args(["up", "--listen", &listen]).arg("--data-dir").arg(&dir);
    // SAFETY: setpgid(0, 0) in the child, right after fork and before exec,
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

/// `GET path` on the collector with the development bearer: the JSON body
/// of a 200, else `None`.
fn get_json(listen: &str, path: &str, token: &str) -> Option<serde_json::Value> {
    let mut stream = TcpStream::connect(listen).ok()?;
```

with:

```rust

/// The owner's password in these tests' collectors.
const PASSWORD: &str = "correct horse battery";

/// Set up the collector whose data directory is `collector_dir` through its
/// `setup-url` (kernel spec §3.1), with `http://<listen>` as `public_url`,
/// and return the session token the setup signed the owner in with.
fn sign_in(listen: &str, collector_dir: &std::path::Path) -> String {
    let file = collector_dir.join("setup-url");
    wait_until("the setup link", || file.exists());
    let url = std::fs::read_to_string(&file).unwrap();
    // `…/setup#<token>`: the token is the fragment (3b decision 16).
    let token = url.trim_end().rsplit_once('#').unwrap().1;
    let origin = format!("http://{listen}");
    let body = serde_json::json!({ "token": token, "password": PASSWORD, "public_url": origin }).to_string();
    let mut stream = TcpStream::connect(listen).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    write!(
        stream,
        "POST /api/setup HTTP/1.1\r\nHost: {listen}\r\nOrigin: {origin}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 201"), "{response}");
    response
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            let value = value.trim().strip_prefix("hennery_session=")?;
            name.eq_ignore_ascii_case("set-cookie")
                .then(|| value.split(';').next().unwrap().to_string())
        })
        .unwrap_or_else(|| panic!("no session cookie: {response}"))
}

/// `GET path` on the collector with the owner's `session`: the JSON body of
/// a 200, else `None`.
fn get_json(listen: &str, path: &str, session: &str) -> Option<serde_json::Value> {
    let mut stream = TcpStream::connect(listen).ok()?;
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )
```

with:

```rust
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nConnection: close\r\n\r\n"
    )
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// Start `up`, wait until its host is connected, and return the connected
/// host ids. The returned guard stops the whole tree (and leaves `dir`).
fn up_until_connected(listen: &str, dir: &std::path::Path) -> (KillTree, Vec<String>) {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
```

with:

```rust
/// Start `up`, wait until its host is connected, and return the connected
/// host ids and the owner's session: `session`, or else a new one from
/// setting the collector up. The returned guard stops the whole tree (and
/// leaves `dir`).
fn up_until_connected(listen: &str, dir: &std::path::Path, session: Option<&str>) -> (KillTree, Vec<String>, String) {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .arg(dir)
        .args(["--dev-token", "dev-token-for-tests"])
        .spawn()
```

with:

```rust
        .arg(dir)
        .spawn()
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(serde_json::Value::Array(hosts)) = get_json(listen, "/api/hosts", "dev-token-for-tests")
            && hosts.iter().any(|h| h["connected"] == true)
```

with:

```rust
    };
    let session = match session {
        Some(session) => session.to_string(),
        None => sign_in(listen, &dir.join("collector")),
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(serde_json::Value::Array(hosts)) = get_json(listen, "/api/hosts", &session)
            && hosts.iter().any(|h| h["connected"] == true)
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
                .collect();
            return (guard, ids);
        }
```

with:

```rust
                .collect();
            return (guard, ids, session);
        }
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

    let (mut first, ids) = up_until_connected(&listen, &dir);
    assert_eq!(ids.len(), 1, "{ids:?}");
```

with:

```rust

    let (mut first, ids, session) = up_until_connected(&listen, &dir, None);
    assert_eq!(ids.len(), 1, "{ids:?}");
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

    let (_second, again) = up_until_connected(&free_listen(), &dir);
    assert_eq!(again, ids, "the restart paired a second host");
```

with:

```rust

    // Set up already: the session from the first run still holds.
    let (_second, again, _) = up_until_connected(&free_listen(), &dir, Some(&session));
    assert_eq!(again, ids, "the restart paired a second host");
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

/// `POST path` with a JSON body, on the collector with the development
/// bearer. The response is never read past a short timeout: a session start
/// that never finishes (the point of the slow-starting-adapter test below)
/// may hold the request open, and the pid files it writes are this test's
/// real readiness signal, not the HTTP response.
fn post_json(listen: &str, path: &str, token: &str, body: &str) {
    let Ok(mut stream) = TcpStream::connect(listen) else {
```

with:

```rust

/// `POST path` with a JSON body, on the collector with the owner's
/// `session`, from its `public_url` (`http://<listen>`). The response is never read past a short timeout: a session start
/// that never finishes (the point of the slow-starting-adapter test below)
/// may hold the request open, and the pid files it writes are this test's
/// real readiness signal, not the HTTP response.
fn post_json(listen: &str, path: &str, session: &str, body: &str) {
    let Ok(mut stream) = TcpStream::connect(listen) else {
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        stream,
        "POST {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
```

with:

```rust
        stream,
        "POST {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: http://{listen}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

/// `DELETE path` on the collector with the development bearer: the status.
fn delete(listen: &str, path: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(listen).ok()?;
```

with:

```rust

/// `DELETE path` on the collector with the owner's `session`, from its
/// `public_url`: the status.
fn delete(listen: &str, path: &str, session: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(listen).ok()?;
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        stream,
        "DELETE {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: Bearer dev-token-for-tests\r\nConnection: close\r\n\r\n"
    )
```

with:

```rust
        stream,
        "DELETE {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: http://{listen}\r\nConnection: close\r\n\r\n"
    )
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .arg(dir)
        .args(["--dev-token", "dev-token-for-tests"])
        .args(extra)
```

with:

```rust
        .arg(dir)
        .args(extra)
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    let mut first = up_logging_to(&listen, &dir.join("data"), &log);
    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", "dev-token-for-tests") else {
            return false;
```

with:

```rust
    let mut first = up_logging_to(&listen, &dir.join("data"), &log);
    let session = sign_in(&listen, &dir.join("data").join("collector"));
    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
            return false;
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    });
    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}")), Some(200));
    wait_until("the revoke logged", || revoked_logged(&log));
    assert!(first.up.try_wait().unwrap().is_none(), "up exited with its host");
    let hosts = get_json(&listen, "/api/hosts", "dev-token-for-tests").expect("the collector still serves");
    assert!(hosts[0]["revoked_at"].is_string(), "{hosts}");
```

with:

```rust
    });
    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}"), &session), Some(200));
    wait_until("the revoke logged", || revoked_logged(&log));
    assert!(first.up.try_wait().unwrap().is_none(), "up exited with its host");
    let hosts = get_json(&listen, "/api/hosts", &session).expect("the collector still serves");
    assert!(hosts[0]["revoked_at"].is_string(), "{hosts}");
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    wait_until("the revoke logged again", || revoked_logged(&log));
    assert!(get_json(&listen, "/api/hosts", "dev-token-for-tests").is_some());
    assert!(second.up.try_wait().unwrap().is_none());
```

with:

```rust
    wait_until("the revoke logged again", || revoked_logged(&log));
    assert!(get_json(&listen, "/api/hosts", &session).is_some());
    assert!(second.up.try_wait().unwrap().is_none());
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .arg(&dir)
        .args(["--dev-token", "dev-token-for-tests"])
        .output()
```

with:

```rust
        .arg(&dir)
        .output()
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .arg(dir.join("data"))
        .args(["--dev-token", "dev-token-for-tests"])
        .args(["--pairing-code-fd", "3"])
```

with:

```rust
        .arg(dir.join("data"))
        .args(["--pairing-code-fd", "3"])
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

    wait_until("the collector serving", || {
        get_json(&listen, "/api/hosts", "dev-token-for-tests").is_some()
    });
```

with:

```rust

    let session = sign_in(&listen, &dir.join("data"));
    wait_until("the collector serving", || {
        get_json(&listen, "/api/hosts", &session).is_some()
    });
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    );

    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", "dev-token-for-tests") else {
            return false;
```

with:

```rust
    );
    let session = sign_in(&listen, &dir.join("data").join("collector"));

    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
            return false;
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        "/api/sessions",
        "dev-token-for-tests",
        &serde_json::json!({ "host_id": host_id, "agent": "slow", "cwd": dir }).to_string(),
```

with:

```rust
        "/api/sessions",
        &session,
        &serde_json::json!({ "host_id": host_id, "agent": "slow", "cwd": dir }).to_string(),
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}")), Some(200));
    // Generous: a reconnect (up to ~1s of backoff) plus `shut_down`'s ~6s
```

with:

```rust

    assert_eq!(delete(&listen, &format!("/api/hosts/{host_id}"), &session), Some(200));
    // Generous: a reconnect (up to ~1s of backoff) plus `shut_down`'s ~6s
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

/// Final review I1: an operator who gives `up` its bearer through the
/// environment (`HENNERY_DEV_TOKEN`, as the flag's `env` allows) must not
/// hand it to the host child, nor through it to any agent. Every other CLI
/// test passes `--dev-token`, which is why nothing caught this before.
///
```

with:

```rust

/// Final review I1: an operator whose shell still exports the old bearer
/// (`HENNERY_DEV_TOKEN`, from before 3b removed it) must not hand it to the
/// host child, nor through it to any agent.
///
```

In `crates/hennery/tests/cli.rs`, replace:

```rust

    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", TOKEN) else {
            return false;
```

with:

```rust

    let session = sign_in(&listen, &dir.join("data").join("collector"));
    let mut host_id = String::new();
    wait_until("the host connected", || {
        let Some(serde_json::Value::Array(hosts)) = get_json(&listen, "/api/hosts", &session) else {
            return false;
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        "/api/sessions",
        TOKEN,
        &serde_json::json!({ "host_id": host_id, "agent": "envdump", "cwd": dir }).to_string(),
```

with:

```rust
        "/api/sessions",
        &session,
        &serde_json::json!({ "host_id": host_id, "agent": "envdump", "cwd": dir }).to_string(),
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// Start `hennery collector` under `umask 022` on `data`, logging to `log`,
/// and wait until it serves.
fn collector_under_umask_022(listen: &str, data: &std::path::Path, log: &std::path::Path) -> KillTree {
```

with:

```rust
/// Start `hennery collector` under `umask 022` on `data`, logging to `log`,
/// and wait until it serves: a new collector writes its setup link once it
/// listens.
fn collector_under_umask_022(listen: &str, data: &std::path::Path, log: &std::path::Path) -> KillTree {
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .arg(data)
        .args(["--dev-token", "dev-token-for-tests"])
        .stdout(std::fs::File::create(log).unwrap())
```

with:

```rust
        .arg(data)
        .stdout(std::fs::File::create(log).unwrap())
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    wait_until("the collector serving", || {
        get_json(listen, "/api/hosts", "dev-token-for-tests").is_some()
    });
```

with:

```rust
    wait_until("the collector serving", || {
        data.join("setup-url").exists() && TcpStream::connect(listen).is_ok()
    });
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .arg(&root)
        .args(["--dev-token", "dev-token-for-tests"])
        .stdout(std::fs::File::create(&log).unwrap())
```

with:

```rust
        .arg(&root)
        .stdout(std::fs::File::create(&log).unwrap())
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    };
    wait_until("the host connected", || {
        get_json(&listen, "/api/hosts", "dev-token-for-tests").is_some_and(|hosts| {
            hosts
```

with:

```rust
    };
    let session = sign_in(&listen, &root.join("collector"));
    wait_until("the host connected", || {
        get_json(&listen, "/api/hosts", &session).is_some_and(|hosts| {
            hosts
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --test auth; cargo test -p hennery --test cli`
Expected: FAIL to compile.
- `error[E0425]: cannot find function operator_client in crate hennery_testkit`;
- `error[E0425]: cannot find value PUBLIC_URL in crate hennery_testkit`;
- `error[E0061]: this function takes 4 arguments but 3 arguments were supplied` (`AppState::new`).

- [ ] **Step 3: The cookie layer, the routes, the binary and the testkit client**

Replace the whole of `crates/hennery-kernel/src/auth.rs` with:

```rust
//! Request auth for operator routes (kernel spec §3.2, §3.3): the session
//! cookie, behind the browser rules. It replaces the walking skeleton's
//! development bearer token.

use crate::auth_api::{error, internal, secure_cookies, with_cookie};
use crate::operator::{Operator, session_cookie, session_token};
use crate::secret::unix_now;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;

/// Put `router`'s routes behind the browser rules (outermost) and the
/// session cookie. Only routes added to `router` before this call are
/// covered: add every operator route first.
pub fn operator_only<S: Clone + Send + Sync + 'static>(router: Router<S>, operator: Arc<Operator>) -> Router<S> {
    router
        .layer(middleware::from_fn_with_state(operator.clone(), require_operator))
        .layer(middleware::from_fn_with_state(operator, crate::origin::browser_rules))
}

/// Axum middleware: the request must carry a live session cookie (401
/// `unauthenticated` otherwise). The session goes into the request's
/// extensions as `Authenticated`; when this request slid its expiry, the
/// cookie is sent again so the browser's copy lives as long.
pub async fn require_operator(State(operator): State<Arc<Operator>>, mut req: Request, next: Next) -> Response {
    let Some(token) = session_token(req.headers()).map(str::to_string) else {
        return unauthenticated();
    };
    let session = match operator.authenticate(&token, unix_now()) {
        Ok(Some(session)) => session,
        Ok(None) => return unauthenticated(),
        Err(err) => return internal(err),
    };
    let slid = session.slid;
    req.extensions_mut().insert(session);
    let response = next.run(req).await;
    if slid {
        with_cookie(response, &session_cookie(&token, secure_cookies(&operator)))
    } else {
        response
    }
}

fn unauthenticated() -> Response {
    error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first")
}
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
use axum::Router;
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

with:

```rust
use axum::Router;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    pub hub: Arc<hub::Hub>,
    pub token: DevToken,
    /// Cancelled on shutdown; long-lived handlers (host sockets, SSE) end
```

with:

```rust
    pub hub: Arc<hub::Hub>,
    /// Cancelled on shutdown; long-lived handlers (host sockets, SSE) end
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
impl AppState {
    pub fn new(store: store::Store, hosts: Hosts, operator: Operator, token: DevToken) -> Self {
        Self {
```

with:

```rust
impl AppState {
    pub fn new(store: store::Store, hosts: Hosts, operator: Operator) -> Self {
        Self {
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
            hub: Arc::new(hub::Hub::new()),
            token,
            shutdown: CancellationToken::new(),
```

with:

```rust
            hub: Arc::new(hub::Hub::new()),
            shutdown: CancellationToken::new(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use futures::stream::{self, Stream, StreamExt};
```

with:

```rust
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/sessions", post(start_session))
```

with:

```rust

/// Every route here is an operator's (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let routes = Router::new()
        .route("/api/sessions", post(start_session))
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .route("/api/sessions/{id}/events", get(events))
        .route("/api/stream/sessions/{id}", get(stream_session))
        .layer(middleware::from_fn_with_state(
            state.token.clone(),
            hennery_kernel::auth::require_bearer,
        ))
        .with_state(state)
}
```

with:

```rust
        .route("/api/sessions/{id}/events", get(events))
        .route("/api/stream/sessions/{id}", get(stream_session));
    hennery_kernel::auth::operator_only(routes, state.operator.clone()).with_state(state)
}
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord, Revoke};
```

with:

```rust
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord, Revoke};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust

/// Routes that need an operator (the development bearer until operator
/// auth replaces it), and enrollment, which is authenticated by its code
/// alone and so sits outside that layer (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let operator = Router::new()
        .route("/api/hosts", get(list_hosts))
        .route("/api/hosts/pairing-codes", post(mint_pairing_code))
        .route("/api/hosts/{id}", delete(revoke_host))
        .layer(middleware::from_fn_with_state(
            state.token.clone(),
            hennery_kernel::auth::require_bearer,
        ));
    let code_authenticated = Router::new().route("/api/hosts/enroll", post(enroll));
```

with:

```rust

/// Routes that need the operator's session, and enrollment, which is
/// authenticated by its code alone and so sits outside that layer (kernel
/// spec §3.3).
pub fn router(state: AppState) -> Router {
    let operator = hennery_kernel::auth::operator_only(
        Router::new()
            .route("/api/hosts", get(list_hosts))
            .route("/api/hosts/pairing-codes", post(mint_pairing_code))
            .route("/api/hosts/{id}", delete(revoke_host)),
        state.operator.clone(),
    );
    let code_authenticated = Router::new().route("/api/hosts/enroll", post(enroll));
```

In `crates/hennery/src/main.rs`, replace:

```rust
//! run) and an all-in-one mode. Hosts authenticate with the key they paired
//! with; the REST API still takes a development bearer token until operator
//! auth lands.

```

with:

```rust
//! run) and an all-in-one mode. Hosts authenticate with the key they paired
//! with; the operator with the session the setup link or a login opened.

```

In `crates/hennery/src/main.rs`, replace:

```rust
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

with:

```rust
use hennery_host::{AgentCommand, HostConfig};
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    data_dir: PathBuf,
    /// Development bearer token for REST clients, until operator auth.
    /// Hosts authenticate with their paired key instead.
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    /// Presume a host's sessions parked once it has been offline this long.
```

with:

```rust
    data_dir: PathBuf,
    /// Presume a host's sessions parked once it has been offline this long.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    data_dir: PathBuf,
    /// The collector's development bearer token for REST clients.
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    #[arg(long = "agent", value_parser = parse_agent)]
```

with:

```rust
    data_dir: PathBuf,
    #[arg(long = "agent", value_parser = parse_agent)]
```

In `crates/hennery/src/main.rs`, replace:

```rust
async fn run_collector(args: CollectorArgs) -> Result<()> {
    // Checked before anything touches the data dir, like `run_up` does.
    let token = DevToken::new(args.dev_token)?;
    private_data_dir(&args.data_dir)?;
```

with:

```rust
async fn run_collector(args: CollectorArgs) -> Result<()> {
    private_data_dir(&args.data_dir)?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let operator = Operator::open(&db)?;
    let mut state = AppState::new(store, hosts, operator, token);
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
```

with:

```rust
    let operator = Operator::open(&db)?;
    let mut state = AppState::new(store, hosts, operator);
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .process_group(0);
    // `up` may have the operator's bearer in its own environment; the host
    // needs none, and its agents must never see it. The adapter spawn strips
    // the same list again, for a host started by hand.
    for var in hennery_host::adapter::HOST_SECRET_VARS {
```

with:

```rust
        .process_group(0);
    // `up` may still have the old development token in its environment (a
    // shell set up before it was removed); the host needs none, and its
    // agents must never see it. The adapter spawn strips the same list
    // again, for a host started by hand.
    for var in hennery_host::adapter::HOST_SECRET_VARS {
```

In `crates/hennery/src/main.rs`, replace:

```rust
async fn run_up(args: UpArgs) -> Result<()> {
    // Checked here too, so a bad token stops `up` before any child starts.
    DevToken::new(args.dev_token.clone())?;
    let exe = std::env::current_exe()?;
```

with:

```rust
async fn run_up(args: UpArgs) -> Result<()> {
    let exe = std::env::current_exe()?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .arg(args.data_dir.join("collector"))
        .env("HENNERY_DEV_TOKEN", &args.dev_token)
        .kill_on_drop(true)
```

with:

```rust
        .arg(args.data_dir.join("collector"))
        .kill_on_drop(true)
```

In `crates/hennery/src/main.rs`, replace:

```rust

    /// Final review I1: `up` may itself have been given the operator's
    /// bearer in its environment (`HENNERY_DEV_TOKEN`); its host child must
    /// not inherit it, so neither can any agent that host runs.
    #[test]
```

with:

```rust

    /// Final review I1: `up` may itself have the old operator bearer in its
    /// environment (`HENNERY_DEV_TOKEN`, left in a shell from before 3b);
    /// its host child must not inherit it, so neither can any agent that
    /// host runs.
    #[test]
```

In `crates/hennery/src/main.rs`, replace:

```rust
            data_dir: "/nonexistent".into(),
            dev_token: "dev-token-for-tests".into(),
            agents: Vec::new(),
```

with:

```rust
            data_dir: "/nonexistent".into(),
            agents: Vec::new(),
```

In `crates/hennery-testkit/Cargo.toml`, replace:

```toml
agent-client-protocol.workspace = true
libc = "0.2"
serde.workspace = true
```

with:

```toml
agent-client-protocol.workspace = true
hennery-kernel.workspace = true
libc = "0.2"
reqwest.workspace = true
serde.workspace = true
```

In `crates/hennery-testkit/Cargo.toml`, replace:

```toml
hennery-host = { workspace = true, features = ["test-hooks"] }
hennery-kernel.workspace = true
hennery-proto.workspace = true
hennery-sessions.workspace = true
hex.workspace = true
reqwest.workspace = true
rusqlite.workspace = true
```

with:

```toml
hennery-host = { workspace = true, features = ["test-hooks"] }
hennery-proto.workspace = true
hennery-sessions.workspace = true
hex.workspace = true
rusqlite.workspace = true
```

In `crates/hennery-testkit/src/lib.rs`, replace:

```rust

/// Environment variable carrying the script.
```

with:

```rust

/// The `public_url` every test collector is set up with, and so the
/// `Origin` its state-changing requests carry.
pub const PUBLIC_URL: &str = "https://hennery.example";

/// The owner's password in test collectors.
pub const OWNER_PASSWORD: &str = "correct horse battery";

/// A client signed in as the owner of `operator`'s collector (kernel spec
/// §3.2), which is set up first if it is not: every request carries the
/// session cookie and the `public_url`'s `Origin`. The session is opened
/// through the operator directly, as a login would.
pub fn operator_client(operator: &hennery_kernel::operator::Operator) -> reqwest::Client {
    let now = hennery_kernel::secret::unix_now();
    if !operator.is_set_up().unwrap() {
        let token = operator.issue_setup_token(now).unwrap().unwrap();
        operator.set_up(&token, OWNER_PASSWORD, PUBLIC_URL, now).unwrap();
    }
    let token = operator.open_session("hennery-testkit", now).unwrap().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::COOKIE,
        format!("{}={token}", hennery_kernel::operator::SESSION_COOKIE)
            .parse()
            .unwrap(),
    );
    headers.insert(reqwest::header::ORIGIN, PUBLIC_URL.parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

/// Environment variable carrying the script.
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p hennery-testkit --locked && cargo test -p hennery --locked`
Expected: all pass. `Cargo.lock` is unchanged: the testkit already had both crates as dev-dependencies.

- [ ] **Step 5: Revert-probe the layering**

In `api::router`, move `.route("/api/stream/sessions/{id}", get(stream_session))` from the `routes` builder to after the `operator_only(...)` call, just before `.with_state(state)`. Rerun `cargo test -p hennery-testkit --test auth --locked`.

Expected: both `every_operator_route_needs_the_session_cookie` and `every_operator_route_applies_the_browser_rules` fail on `GET /api/stream/sessions/s-1`, which answers `(200, "")`. Every request there is bounded at 10 s, so a streaming route that escaped fails the test instead of hanging it. Restore the code.

Then, in `browser_rules` (Task 4's code, this task's test), change `if site.is_some_and(|s| s != "same-origin" && s != "none") {` to `if false && site.is_some_and(|s| s != "same-origin" && s != "none") {`. Expected: `every_operator_route_applies_the_browser_rules` fails. Restore the code.

- [ ] **Step 6: Check the CLI tests under load**

Run: `cargo test -p hennery --test cli --no-run --locked`, then run the printed `cli-…` binary four times at once (with the working directory `crates/hennery`), three times over.
Expected: every run passes.

- [ ] **Step 7: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 424 tests pass, as many as before: four added in `auth.rs` and one in `cli.rs`; the bearer test, the two `DevToken` unit tests and the two short-token CLI tests removed (decision 11).

- [ ] **Step 8: Commit and push**

```bash
git add crates
git commit -m "feat(sessions): the session cookie replaces the development bearer on every operator route"
git push
```

### Task 6: Step-up for minting, host revoke and session revoke; the session list; streams end with their session

**Files:**
- Modify: `crates/hennery-kernel/Cargo.toml` (`time`), `Cargo.lock`, `crates/hennery-kernel/src/auth.rs` (`require_step_up`, `session_ended`), `crates/hennery-kernel/src/auth_api.rs`, `crates/hennery-kernel/src/operator.rs` (`step_up_limiter`, the ending generation), `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, `crates/hennery-sessions/src/hosts.rs`, `crates/hennery-sessions/src/api.rs` (the stream)
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-testkit/tests/step_up.rs`, `crates/hennery-testkit/tests/auth.rs` (three more routes in the table)

**Interfaces:**
- Consumes: Tasks 2, 4 and 5 (`step_up`, `sessions`, `revoke_session`, `login_limiter`, `operator_only`, `Authenticated` in the extensions).
- Produces (`hennery_kernel::auth`):
  - `async fn require_step_up(Request, Next) -> Response`, which answers 403 `step_up_required`. It is used as a `route_layer` inside `operator_only`.
  - `async fn session_ended(Arc<Operator>, Authenticated)`, which resolves once that session is revoked, signed out or expired.
- Produces (`hennery_kernel::operator`):
  - the field `step_up_limiter: Limiter`, with `Policy::LOGIN`;
  - `session_ends() -> tokio::sync::watch::Receiver<u64>`, bumped by every `revoke_session` that ends a session;
  - `session_expires_at(session_id: &str, now) -> Result<Option<i64>>`, which does not slide.
- Produces (`hennery_proto::rest`):
  - `StepUpRequest { password: String }`, with a redacting `Debug`;
  - `AuthSessionItem { id, user_agent, created_at, last_seen_at, expires_at: String, current: bool }`.
- Produces (HTTP), all behind `operator_only`:
  - `POST /api/auth/step-up/password`: 204, 401 `invalid_password`, or 429;
  - `GET /api/auth/sessions`: `Vec<AuthSessionItem>`;
  - `DELETE /api/auth/sessions/{id}` (step-up): 204 or 404;
  - `POST /api/hosts/pairing-codes` and `DELETE /api/hosts/{id}` now need step-up too;
  - `GET /api/stream/sessions/{id}` ends when the session that opened it ends.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-testkit/tests/step_up.rs`:

```rust
//! Step-up (kernel spec §3.4, §11) and the signed-in sessions (§3.2):
//! minting a pairing code, revoking a host and revoking a session need a
//! password check within the last five minutes, and are refused without
//! one, accepted within five minutes, and refused after.

use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{ApiError, AuthSessionItem, StepUpRequest};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use hennery_testkit::{OWNER_PASSWORD, PUBLIC_URL};
use std::net::SocketAddr;
use std::time::Duration;

struct Collector {
    addr: SocketAddr,
    state: AppState,
}

impl Collector {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open_in_memory().unwrap(),
            Hosts::open_in_memory().unwrap(),
            Operator::open_in_memory().unwrap(),
        );
        // Sets the collector up.
        hennery_testkit::operator_client(&state.operator);
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state }
    }

    /// A session whose last password check was `age` seconds ago.
    fn session(&self, age: i64) -> String {
        self.state
            .operator
            .open_session("test", unix_now() - age)
            .unwrap()
            .unwrap()
    }

    fn request(&self, session: &str, method: &str, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method.parse().unwrap(), format!("http://{}{path}", self.addr))
            .header("origin", PUBLIC_URL)
            .header("cookie", format!("hennery_session={session}"))
    }

    async fn step_up(&self, session: &str, password: &str) -> reqwest::Response {
        self.request(session, "POST", "/api/auth/step-up/password")
            .json(&StepUpRequest {
                password: password.into(),
            })
            .send()
            .await
            .unwrap()
    }

    fn id_of(&self, session: &str) -> String {
        self.state
            .operator
            .authenticate(session, unix_now())
            .unwrap()
            .unwrap()
            .session_id
    }
}

async fn code_of(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    (
        status,
        resp.json::<ApiError>().await.map(|e| e.code).unwrap_or_default(),
    )
}

/// Kernel spec §3.4 and §11: every listed action refused without a fresh
/// check, accepted within five minutes, and refused after.
#[tokio::test]
async fn minting_revoking_a_host_and_revoking_a_session_need_a_fresh_password_check() {
    let c = Collector::start().await;
    let other = c.session(0);
    let actions = [
        ("POST", "/api/hosts/pairing-codes".to_string(), 201),
        ("DELETE", "/api/hosts/host-9".to_string(), 404),
        ("DELETE", format!("/api/auth/sessions/{}", c.id_of(&other)), 204),
    ];
    // Five minutes after the last check: refused, and nothing happens.
    let stale = c.session(5 * 60);
    for (method, path, _) in &actions {
        let resp = c.request(&stale, method, path).send().await.unwrap();
        assert_eq!(code_of(resp).await, (403, "step_up_required".into()), "{method} {path}");
    }
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_some());
    // A wrong password does not step up.
    assert_eq!(
        code_of(c.step_up(&stale, "wrong password").await).await,
        (401, "invalid_password".into())
    );
    assert_eq!(
        code_of(
            c.request(&stale, "POST", "/api/hosts/pairing-codes")
                .send()
                .await
                .unwrap()
        )
        .await,
        (403, "step_up_required".into())
    );
    // The right one does, for this session.
    assert_eq!(c.step_up(&stale, OWNER_PASSWORD).await.status(), 204);
    for (method, path, status) in &actions {
        let resp = c.request(&stale, method, path).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), *status, "{method} {path}");
    }
    assert!(c.state.operator.authenticate(&other, unix_now()).unwrap().is_none());
    // Just inside the five minutes still counts.
    let recent = c.session(5 * 60 - 5);
    let resp = c
        .request(&recent, "POST", "/api/hosts/pairing-codes")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
}

impl Collector {
    async fn login(&self, password: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("http://{}/api/auth/login", self.addr))
            .header("origin", PUBLIC_URL)
            .json(&serde_json::json!({ "password": password }))
            .send()
            .await
            .unwrap()
    }
}

/// 3b decision 10: step-up has a budget of its own. Five wrong step-ups
/// lock out step-up from that address, even with the right password, and
/// leave login alone.
#[tokio::test]
async fn wrong_step_ups_lock_out_step_up_only() {
    let c = Collector::start().await;
    let session = c.session(0);
    for _ in 0..5 {
        assert_eq!(c.step_up(&session, "wrong password").await.status(), 401);
    }
    assert_eq!(
        code_of(c.step_up(&session, OWNER_PASSWORD).await).await,
        (429, "rate_limited".into())
    );
    assert_eq!(c.login(OWNER_PASSWORD).await.status(), 204);
}

/// The other way round: a login flood from the owner's address (a shared
/// proxy, say) does not stop a signed-in owner stepping up.
#[tokio::test]
async fn a_locked_out_login_does_not_block_step_up() {
    let c = Collector::start().await;
    for _ in 0..5 {
        assert_eq!(c.login("wrong password").await.status(), 401);
    }
    assert_eq!(
        code_of(c.login(OWNER_PASSWORD).await).await,
        (429, "rate_limited".into())
    );
    let session = c.session(0);
    assert_eq!(c.step_up(&session, OWNER_PASSWORD).await.status(), 204);
}

/// Open `GET /api/stream/sessions/s-1` with `session`.
async fn open_stream(c: &Collector, session: &str) -> reqwest::Response {
    let resp = c
        .request(session, "GET", "/api/stream/sessions/s-1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    resp
}

/// Whether the SSE body of `stream` ends within `within`.
async fn ends(stream: reqwest::Response, within: Duration) -> bool {
    use futures::StreamExt;
    let mut body = stream.bytes_stream();
    tokio::time::timeout(within, async { while let Some(Ok(_)) = body.next().await {} })
        .await
        .is_ok()
}

/// 3b decision 7: a stream a session opened ends when that session does,
/// by a revoke or a logout; another session's stream stays open.
#[tokio::test]
async fn ending_a_session_ends_its_open_streams() {
    let c = Collector::start().await;
    let owner = c.session(0);
    let kept = open_stream(&c, &owner).await;

    let revoked = c.session(0);
    let stream = open_stream(&c, &revoked).await;
    let resp = c
        .request(&owner, "DELETE", &format!("/api/auth/sessions/{}", c.id_of(&revoked)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a revoked session's stream stayed open"
    );

    let signed_out = c.session(0);
    let stream = open_stream(&c, &signed_out).await;
    let resp = c.request(&signed_out, "POST", "/api/auth/logout").send().await.unwrap();
    assert_eq!(resp.status(), 204);
    assert!(
        ends(stream, Duration::from_secs(1)).await,
        "a signed-out session's stream stayed open"
    );

    // Control: the owner's own stream is still open.
    assert!(
        !ends(kept, Duration::from_millis(200)).await,
        "an unrelated stream ended"
    );
}

/// A logged request must never show the password.
#[test]
fn a_step_up_request_does_not_show_its_password_in_debug() {
    let shown = format!(
        "{:?}",
        StepUpRequest {
            password: OWNER_PASSWORD.into()
        }
    );
    assert!(!shown.contains(OWNER_PASSWORD), "{shown}");
}

#[tokio::test]
async fn the_session_list_marks_the_current_one_and_revoking_it_signs_out() {
    let c = Collector::start().await;
    let mine = c.session(0);
    let listed: Vec<AuthSessionItem> = c
        .request(&mine, "GET", "/api/auth/sessions")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // `operator_client`'s session and this one.
    assert_eq!(listed.len(), 2);
    let current: Vec<_> = listed.iter().filter(|s| s.current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, c.id_of(&mine));
    assert_eq!(current[0].user_agent, "test");
    assert!(!listed.iter().any(|s| s.id == mine), "a token was listed");

    let unknown = c
        .request(&mine, "DELETE", "/api/auth/sessions/0000")
        .send()
        .await
        .unwrap();
    assert_eq!(code_of(unknown).await, (404, "not_found".into()));
    let own = c
        .request(&mine, "DELETE", &format!("/api/auth/sessions/{}", current[0].id))
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), 204);
    let cookie = own.headers()["set-cookie"].to_str().unwrap();
    assert!(
        cookie.starts_with("hennery_session=;") && cookie.contains("Max-Age=0"),
        "{cookie}"
    );
    let after = c.request(&mine, "GET", "/api/auth/sessions").send().await.unwrap();
    assert_eq!(code_of(after).await, (401, "unauthenticated".into()));
}
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
    ("GET", "/api/stream/sessions/s-1"),
];
```

with:

```rust
    ("GET", "/api/stream/sessions/s-1"),
    ("POST", "/api/auth/step-up/password"),
    ("GET", "/api/auth/sessions"),
    ("DELETE", "/api/auth/sessions/0000"),
];
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --test step_up; cargo test -p hennery-testkit --test auth`
Expected:
- `step_up` fails to compile: `error[E0432]: unresolved imports hennery_proto::rest::AuthSessionItem, hennery_proto::rest::StepUpRequest`.
- `auth` fails: `every_operator_route_needs_the_session_cookie` and `every_operator_route_applies_the_browser_rules` get 404 from the three routes that do not exist yet.

- [ ] **Step 3: Step-up, the session list, the guarded routes and the stream's end**

In `crates/hennery-kernel/Cargo.toml`, replace:

```toml
sha2.workspace = true
tokio.workspace = true
```

with:

```toml
sha2.workspace = true
time.workspace = true
tokio.workspace = true
```

In `crates/hennery-kernel/src/auth.rs`, replace:

```rust
use crate::auth_api::{error, internal, secure_cookies, with_cookie};
use crate::operator::{Operator, session_cookie, session_token};
use crate::secret::unix_now;
```

with:

```rust
use crate::auth_api::{error, internal, secure_cookies, with_cookie};
use crate::operator::{Authenticated, Operator, session_cookie, session_token};
use crate::secret::unix_now;
```

In `crates/hennery-kernel/src/auth.rs`, replace:

```rust
use std::sync::Arc;

```

with:

```rust
use std::sync::Arc;
use std::time::Duration;

```

Append to `crates/hennery-kernel/src/auth.rs`:

```rust
/// Axum middleware, inside `require_operator`: the session's last password
/// check must be within the last five minutes (kernel spec §3.4), or the
/// answer is 403 `step_up_required` and the client asks again.
pub async fn require_step_up(req: Request, next: Next) -> Response {
    let fresh = req
        .extensions()
        .get::<Authenticated>()
        .is_some_and(|session| session.stepped_up(unix_now()));
    if !fresh {
        return error(
            StatusCode::FORBIDDEN,
            "step_up_required",
            "confirm your password again (POST /api/auth/step-up/password)",
        );
    }
    next.run(req).await
}

/// Resolves once `session` has ended: revoked, signed out, or expired. A
/// stream a session holds open must not outlive it (3b decision 7), so
/// long-lived responses end with this. It re-checks the session whenever
/// the operator announces an ending, and at its expiry (which may have
/// slid since, through the session's other requests).
pub async fn session_ended(operator: Arc<Operator>, session: Authenticated) {
    let mut ends = operator.session_ends();
    // Whatever ended before this subscription is checked here.
    let Ok(Some(mut expires_at)) = operator.session_expires_at(&session.session_id, unix_now()) else {
        return;
    };
    loop {
        let left = Duration::from_secs(expires_at.saturating_sub(unix_now()).max(0) as u64);
        tokio::select! {
            changed = ends.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = tokio::time::sleep(left) => {}
        }
        match operator.session_expires_at(&session.session_id, unix_now()) {
            Ok(Some(later)) => expires_at = later,
            Ok(None) | Err(_) => return,
        }
    }
}
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup, and
//! login and logout.

use crate::operator::{Operator, PublicUrl, SetupOutcome, cleared_cookie, session_cookie, session_token};
use crate::secret::unix_now;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, LoginRequest, SetupRequest, SetupResponse};
use std::net::SocketAddr;
```

with:

```rust
//! Operator auth over HTTP (kernel spec §3, §8): the one-time setup, login
//! and logout, step-up, and the signed-in sessions.

use crate::operator::{
    Authenticated, Operator, PublicUrl, SetupOutcome, cleared_cookie, session_cookie, session_token,
};
use crate::secret::unix_now;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use hennery_proto::rest::{ApiError, AuthSessionItem, LoginRequest, SetupRequest, SetupResponse, StepUpRequest};
use std::net::SocketAddr;
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
        ));
    let private = || middleware::map_response(crate::setup_page::private_headers);
```

with:

```rust
        ));
    let signed_in = crate::auth::operator_only(
        Router::new()
            .route("/api/auth/step-up/password", post(step_up))
            .route("/api/auth/sessions", get(list_sessions))
            .route(
                "/api/auth/sessions/{id}",
                delete(revoke_session).route_layer(middleware::from_fn(crate::auth::require_step_up)),
            ),
        operator.clone(),
    );
    let private = || middleware::map_response(crate::setup_page::private_headers);
```

In `crates/hennery-kernel/src/auth_api.rs`, replace:

```rust
        .merge(browser)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
```

with:

```rust
        .merge(browser)
        .merge(signed_in)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
```

Append to `crates/hennery-kernel/src/auth_api.rs`:

```rust
/// `POST /api/auth/step-up/password`: 204, the session stepped up for five
/// minutes (kernel spec §3.4). Rate limited like login, on a budget of its
/// own (3b decision 10).
async fn step_up(
    State(operator): State<Arc<Operator>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Extension(session): Extension<Authenticated>,
    Json(req): Json<StepUpRequest>,
) -> Response {
    if let Err(retry_after) = operator.step_up_limiter.attempt(peer.ip(), Instant::now()) {
        return rate_limited(
            retry_after,
            "too many wrong passwords from this address; try again later",
        );
    }
    match operator.check_password(req.password).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::UNAUTHORIZED, "invalid_password", "wrong password"),
        Err(err) => return internal(err),
    }
    operator.step_up_limiter.succeeded(peer.ip());
    match operator.step_up(&session.session_id, unix_now()) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::UNAUTHORIZED, "unauthenticated", "sign in first"),
        Err(err) => internal(err),
    }
}

/// RFC 3339 for a kernel timestamp (seconds since the epoch).
fn rfc3339(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_default()
}

/// `GET /api/auth/sessions`: every signed-in session, most recently used
/// first, the request's own marked `current`.
async fn list_sessions(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
) -> Response {
    match operator.sessions(unix_now()) {
        Ok(sessions) => {
            let items: Vec<AuthSessionItem> = sessions
                .into_iter()
                .map(|s| AuthSessionItem {
                    current: s.id == session.session_id,
                    id: s.id,
                    user_agent: s.user_agent,
                    created_at: rfc3339(s.created_at),
                    last_seen_at: rfc3339(s.last_seen_at),
                    expires_at: rfc3339(s.expires_at),
                })
                .collect();
            Json(items).into_response()
        }
        Err(err) => internal(err),
    }
}

/// `DELETE /api/auth/sessions/{id}` (step-up): 204, that session signed
/// out; 404 if there is none. Ending the request's own session also clears
/// its cookie.
async fn revoke_session(
    State(operator): State<Arc<Operator>>,
    Extension(session): Extension<Authenticated>,
    Path(id): Path<String>,
) -> Response {
    match operator.revoke_session(&id) {
        Ok(true) if id == session.session_id => with_cookie(
            StatusCode::NO_CONTENT.into_response(),
            &cleared_cookie(secure_cookies(&operator)),
        ),
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "not_found", "no such session"),
        Err(err) => internal(err),
    }
}
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
    verifications: AtomicU64,
    /// Wrong passwords per client address, at login and step-up (kernel
    /// spec §3.2).
    pub login_limiter: Limiter,
}
```

with:

```rust
    verifications: AtomicU64,
    /// Wrong passwords per client address at login (kernel spec §3.2).
    pub login_limiter: Limiter,
    /// Wrong passwords per client address at step-up, a budget of its own
    /// (3b decision 10): a login flood from a shared address does not stop
    /// a signed-in owner stepping up, nor step-up guesses lock out login.
    pub step_up_limiter: Limiter,
    /// Bumped whenever a session ends (revoked or signed out): streams
    /// held open by a session re-check it on every bump (3b decision 7).
    ended: tokio::sync::watch::Sender<u64>,
}
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            login_limiter: Limiter::new(Policy::LOGIN),
        })
```

with:

```rust
            login_limiter: Limiter::new(Policy::LOGIN),
            step_up_limiter: Limiter::new(Policy::LOGIN),
            ended: tokio::sync::watch::Sender::new(0),
        })
```

In `crates/hennery-kernel/src/operator.rs`, replace:

```rust
            .execute("DELETE FROM auth_sessions WHERE id_hash = ?1", [session_id])?;
        Ok(changed > 0)
    }
```

with:

```rust
            .execute("DELETE FROM auth_sessions WHERE id_hash = ?1", [session_id])?;
        if changed > 0 {
            self.ended.send_modify(|generation| *generation += 1);
        }
        Ok(changed > 0)
    }

    /// Changes whenever a session ends (`revoke_session`, and so logout).
    pub fn session_ends(&self) -> tokio::sync::watch::Receiver<u64> {
        self.ended.subscribe()
    }

    /// When the live session `session_id` expires, without sliding it;
    /// `None` once it is gone or expired.
    pub fn session_expires_at(&self, session_id: &str, now: i64) -> Result<Option<i64>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT expires_at FROM auth_sessions WHERE id_hash = ?1 AND expires_at > ?2",
                params![session_id, now],
                |r| r.get(0),
            )
            .optional()?)
    }
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// `POST /api/auth/step-up/password` (kernel spec §3.4): the owner's
/// password again, within a session. 204 on success. `Debug` leaves the
/// password out.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct StepUpRequest {
    pub password: String,
}

impl std::fmt::Debug for StepUpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepUpRequest").finish_non_exhaustive()
    }
}

/// One entry of `GET /api/auth/sessions` (kernel spec §3.2): a signed-in
/// device, most recently used first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct AuthSessionItem {
    /// What `DELETE /api/auth/sessions/{id}` takes: the SHA-256 of the
    /// session's token, never the token.
    pub id: String,
    pub user_agent: String,
    /// RFC 3339.
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    /// The session this request came with.
    pub current: bool,
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SetupResponse,
        rest::LoginRequest,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::SetupResponse,
        rest::LoginRequest,
        rest::StepUpRequest,
        rest::AuthSessionItem,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::LoginRequest,
    );
```

with:

```rust
        rest::LoginRequest,
        rest::StepUpRequest,
        rest::AuthSessionItem,
    );
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord, Revoke};
```

with:

```rust
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord, Revoke};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust

/// Routes that need the operator's session, and enrollment, which is
/// authenticated by its code alone and so sits outside that layer (kernel
```

with:

```rust

/// Routes that need the operator's session (minting a code and revoking a
/// host a fresh step-up too, kernel spec §3.4), and enrollment, which is
/// authenticated by its code alone and so sits outside that layer (kernel
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
            .route("/api/hosts", get(list_hosts))
            .route("/api/hosts/pairing-codes", post(mint_pairing_code))
            .route("/api/hosts/{id}", delete(revoke_host)),
        state.operator.clone(),
```

with:

```rust
            .route("/api/hosts", get(list_hosts))
            .route(
                "/api/hosts/pairing-codes",
                post(mint_pairing_code).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
            )
            .route(
                "/api/hosts/{id}",
                delete(revoke_host).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up)),
            ),
        state.operator.clone(),
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use crate::store::{AnswerSubmission, ResumeRequest, Store};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
```

with:

```rust
use crate::store::{AnswerSubmission, ResumeRequest, Store};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
use futures::stream::{self, Stream, StreamExt};
use hennery_proto::frames::{Capability, CollectorFrame, Indexed, SessionBody};
```

with:

```rust
use futures::stream::{self, Stream, StreamExt};
use hennery_kernel::operator::Authenticated;
use hennery_proto::frames::{Capability, CollectorFrame, Indexed, SessionBody};
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
/// Session stream: replays from `Last-Event-ID`, then follows live events.
async fn stream_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
```

with:

```rust
/// Session stream: replays from `Last-Event-ID`, then follows live events.
/// The session's events as SSE, until the collector shuts down or the
/// operator's session that opened it ends (3b decision 7).
async fn stream_session(
    State(state): State<AppState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
        .chain(follow)
        .take_until(state.shutdown.clone().cancelled_owned());
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
```

with:

```rust
        .chain(follow)
        .take_until(state.shutdown.clone().cancelled_owned())
        .take_until(hennery_kernel::auth::session_ended(
            state.operator.clone(),
            operator_session,
        ));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
```

- [ ] **Step 4: Update the lock file, regenerate and run the tests**

Run: `cargo build --workspace` (no `--locked`: it records `time` for the kernel), `cargo run -p hennery-proto --bin gen`, then `cargo test -p hennery-testkit --test step_up --test auth --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probe the step-up layers, the separate budget, the stream's end and `Debug`**

1. In `hosts::router`, change `post(mint_pairing_code).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up))` to `post(mint_pairing_code)`. Rerun `cargo test -p hennery-testkit --test step_up --locked`. Expected: `minting_revoking_a_host_and_revoking_a_session_need_a_fresh_password_check` fails, `(201, "")` where 403 was expected. Restore the code.
2. In `auth_api::router`, change `delete(revoke_session).route_layer(middleware::from_fn(crate::auth::require_step_up))` to `delete(revoke_session)`. Expected: the same test fails. Restore the code.
3. In `hosts::router`, change `delete(revoke_host).route_layer(middleware::from_fn(hennery_kernel::auth::require_step_up))` to `delete(revoke_host)`. Expected: the same test fails. Restore the code.
4. In `step_up`, use `operator.login_limiter` in place of `operator.step_up_limiter`, in both places. Expected: `wrong_step_ups_lock_out_step_up_only` and `a_locked_out_login_does_not_block_step_up` both fail. Restore the code.
5. In `stream_session`, remove the `.take_until(hennery_kernel::auth::session_ended(…))` call. Expected: `ending_a_session_ends_its_open_streams` fails after its one-second wait. Restore the code.
6. In `Operator::revoke_session`, remove `self.ended.send_modify(|generation| *generation += 1);`. Expected: the same test fails. Restore the code.
7. In `StepUpRequest`'s `Debug`, add `.field("password", &self.password)`. Expected: `a_step_up_request_does_not_show_its_password_in_debug` fails. Restore the code.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run: `cargo test -p hennery-testkit --test step_up --test auth --test login --test setup --no-run --locked`, then run each printed binary four times at once, three times over.
Expected: every run passes. `ending_a_session_ends_its_open_streams` waits on the stream's end with a one-second bound; it never sleeps for the ending itself.

- [ ] **Step 7: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 430 tests pass. The CLI's revoke tests pass without an explicit step-up: the session the setup opened is stepped up (decision 3).

- [ ] **Step 8: Commit and push**

```bash
git add Cargo.lock crates schema web/src/generated
git commit -m "feat(kernel): step-up for minting, host revoke and session revoke; the session list; streams end with their session"
git push
```

### Task 7: `host join` takes the pairing code on standard input

**Files:**
- Modify: `crates/hennery/src/main.rs`
- Test: `crates/hennery/tests/cli.rs`

**Interfaces:**
- Produces (binary): `hennery host join <URL> [CODE]`. Without `CODE`, one line is read from stdin, with a prompt on stderr when stdin is a terminal. An empty line fails with `no pairing code: give it after the URL, or on standard input`.

- [ ] **Step 1: Write the failing test**

Append to `crates/hennery/tests/cli.rs`:

```rust
/// 3a's deferred M5: a pairing code on the command line is in the process
/// list and the shell history, so `host join` also takes it on standard
/// input when it is left out, and refuses an empty one.
#[test]
fn join_reads_the_code_from_standard_input_when_it_is_left_out() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = std::env::temp_dir().join(format!("hennery-cli-stdin-code-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = RemoveDir(dir.clone());
    let (addr, code) = rt.block_on(async {
        let db = dir.join("hennery.db");
        let state = hennery_sessions::AppState::new(
            hennery_sessions::store::Store::open(&db).unwrap(),
            hennery_kernel::hosts::Hosts::open(&db).unwrap(),
            hennery_kernel::operator::Operator::open(&db).unwrap(),
        );
        let code = state
            .hosts
            .mint_pairing_code(hennery_kernel::secret::unix_now())
            .unwrap()
            .code;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(hennery_sessions::serve(listener, state));
        (addr, code)
    });
    let join = |stdin: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hennery"))
            .args(["host", "join", &format!("http://{addr}"), "--name", "laptop"])
            .arg("--data-dir")
            .arg(dir.join("host"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    };

    let empty = join("\n");
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("no pairing code"));

    let out = join(&format!("{code}\n"));
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("paired as"));
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p hennery --test cli join_reads`
Expected: FAIL. `join_reads_the_code_from_standard_input_when_it_is_left_out` panics: clap refuses the missing `<CODE>`, so stderr does not say `no pairing code`.

- [ ] **Step 3: The optional code**

In `crates/hennery/src/main.rs`, replace:

```rust
    url: String,
    /// The pairing code shown by the collector (`XXXX-XXXX`).
    code: String,
    /// How the collector lists this host; defaults to the host name.
```

with:

```rust
    url: String,
    /// The pairing code shown by the collector (`XXXX-XXXX`). Leave it out
    /// to type it, or pipe it, on standard input instead: that keeps it out
    /// of the process list and the shell history.
    code: Option<String>,
    /// How the collector lists this host; defaults to the host name.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let name = args.name.unwrap_or_else(hennery_host::pairing::default_name);
    match hennery_host::pairing::join(&args.url, &args.code, &args.data_dir, &name).await? {
        Joined::Paired { host_id } => println!("paired as {host_id}"),
```

with:

```rust
    let name = args.name.unwrap_or_else(hennery_host::pairing::default_name);
    let code = match args.code {
        Some(code) => code,
        None => read_code_from_stdin()?,
    };
    match hennery_host::pairing::join(&args.url, &code, &args.data_dir, &name).await? {
        Joined::Paired { host_id } => println!("paired as {host_id}"),
```

In `crates/hennery/src/main.rs`, replace:

```rust
        Joined::AlreadyPaired { host_id } => println!("already paired as {host_id}; nothing to do"),
    }
    Ok(())
}

async fn run_host(args: HostArgs) -> Result<std::process::ExitCode> {
```

with:

```rust
        Joined::AlreadyPaired { host_id } => println!("already paired as {host_id}; nothing to do"),
    }
    Ok(())
}

/// One line of standard input, prompted for on a terminal.
fn read_code_from_stdin() -> Result<String> {
    use std::io::{BufRead, IsTerminal, Write};
    if std::io::stdin().is_terminal() {
        eprint!("Pairing code: ");
        std::io::stderr().flush()?;
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("read the pairing code from standard input")?;
    let code = line.trim().to_string();
    if code.is_empty() {
        bail!("no pairing code: give it after the URL, or on standard input");
    }
    Ok(code)
}

async fn run_host(args: HostArgs) -> Result<std::process::ExitCode> {
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p hennery --test cli join --locked`
Expected: `host_help_lists_join_and_run`, `joining_over_http_ignores_a_configured_proxy` and the new test pass.

- [ ] **Step 5: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check && cargo run --bin gen -- --check`
Expected: all 431 tests pass.

- [ ] **Step 6: Commit and push**

```bash
git add crates/hennery
git commit -m "feat(cli): host join takes the pairing code on standard input"
git push
```

## After this plan

**Plan 3b-ii, the collector's configuration surface** (kernel §1, §2, §4.2, §7):
- **Several listeners** (maintainer decision 6c):
  - `listen = […]` in `config.toml`, a repeatable `--listen`, and a comma-separated `HENNERY_LISTEN`;
  - every listener gets the same router, `ConnectInfo`, browser rules and cookie;
  - start fails if any address cannot be bound;
  - browser access stays bound to `public_url`;
  - the route-table tests (`auth.rs`) run on every listener.
- **`config.toml`**, and the precedence of flags, then environment, then file, then defaults. Also a `public_url` there, so the setup link can name it (kernel §3.1: `<public_url or http://localhost:PORT>`).
- **The admin socket** (`admin.sock`, 0600):
  - print the setup URL: it needs `Operator::announce_setup`'s link, which lives in memory;
  - reset the password: a new `set_password` on `Operator`, which also ends every session and bumps the ending generation (decision 7), so their streams end too;
  - **reset `public_url`** (decision 4's lock-out): an admin-socket command, or a `config.toml` value that overrides the stored one. Either path must replace `Operator`'s cached `public_url`, not only the `settings` row;
  - list hosts; mint a pairing code.

  Destructive commands need a confirmation on a TTY.
- **`owner_id` on the older tables** (decision 1): `hosts`, `pairing_codes` and every sessions-store table, backfilled with the one owner, with every query filtering by it. That includes the operator's own queries, which select across all owners today: `verify_password`, `authenticate`, `step_up`, `sessions`, `revoke_session` and `load_public_url` (and `session_expires_at`).
- **`/healthz` and `/readyz`**, exempt (kernel §3.3). A test pins that they carry no data.

**Plan 3c, passkeys** (unchanged): `webauthn-rs`, with the RP id and origin from `public_url` and ceremony state in memory. Several labelled passkeys, removable while another login method remains. Step-up by passkey. Tests with the `passkey` crate's software authenticator. `webauthn-rs` needs OpenSSL: add `openssl` and `pkg-config` to `flake.nix`'s dev shell (no global install), and vendor it statically for the musl build.

**Obligations this plan hands on:**
- **Step-up on the rest of §3.4**, as each endpoint arrives:
  - gateway connection URLs, credentials, pre-registered clients and the "internal network" flag;
  - local stdio servers;
  - session delete;
  - hat purge;
  - changing `public_url` (`PATCH /api/settings`).

  Each gets `.route_layer(require_step_up)` and a row in `step_up.rs`.
- **The frontend:**
  - the setup page at `/setup`, replacing `setup_page.rs`: it reads the token from the fragment and POSTs `SetupRequest` (decision 16);
  - the login form;
  - the `step_up_required` prompt and retry (frontend spec §3);
  - Settings' session list, rendering `user_agent` escaped.
- **The default hat's name** in `SetupRequest` (decision 3), with hats.
- **The login limiter behind a proxy** (decision 6): a flood through a reverse proxy locks every client behind it out of password login for up to an hour, since they share one address. What still works:
  - a collector restart clears the lockout (the limits live in memory);
  - a signed-in owner can still step up (decision 10);
  - passkey login (3c) must not be gated by the password limiter's lockout.

  If lockouts are seen in practice, the options are OWASP-style device cookies (a browser that logged in before keeps its own budget), or a signed forwarded identity (deferred by the spec).
- **The collector's single writer thread** (kernel §1). Decision 14 serialises today's three writers with `IMMEDIATE` transactions; one writer would also remove the busy waits.
- **A race found while building this plan, not caused by it, to fix on its own, separately from auth:**
  - The race: `up` installs its signal handlers only after it has spawned both children. A SIGTERM before then kills `up` by the default action and leaves the collector child orphaned (reparented to launchd, still running).
  - How it shows: `up_warns_about_a_loose_existing_data_root` sends SIGTERM right after `up`'s data-root warning, so it can hit that window. It left one orphaned collector in about ten full runs.
  - The fix: register the `SIGINT` and `SIGTERM` streams at the top of `run_up`, before any spawn, and wait on those streams in the select loop.
- **The CLI tests' port race, PROVEN:** under parallel load, CLI tests time out waiting for the setup link or for the collector to serve, because `free_listen`'s port is taken between its release and the collector's bind (`collector.err`: `Address already in use`). A harness PR, separate from auth, should fix it (for example, the collector binds port 0 and the test reads the port from `setup-url`), and also cover:
  - the macOS pipe `CLOEXEC` race suspected in `a_failed_pairing_code_write_is_logged_without_the_code_and_does_not_kill_the_collector` (`pipe` then `fcntl` is not atomic there);
  - `wait_until` calling `try_wait` on the child and printing its `.err`, so a dead collector fails the test at once and says why.
- **Hardening the review suggested and the coordinator skipped for now:**
  - the `__Host-` cookie prefix, which renames the cookie kernel §3.2 fixes as `hennery_session`;
  - revoking a live session that is presented to login, in place of opening a second one;
  - a dummy hash precomputed at start, not on the first check before setup.
- **`is_json_or_empty` and HTTP/2** (decision 8). The rule reads a body-less request from `Content-Length` (absent or 0) and a missing `Transfer-Encoding`. Over HTTP/2, which forbids `Transfer-Encoding`, a body can arrive with neither header, so a body with no `Content-Type` passes the rule. Two things hold it today: every handler that reads a body takes `Json`, which refuses a missing `Content-Type` on its own (415), and the body-less handlers never read one. If a handler ever reads a raw body, check it for itself.
- **Found in execution and the final review, not fixed here:**
  - `verify_password` is public and takes no hashing permit. Any later hash outside setup and login (3b-ii's password reset or change) MUST go through the permit (`check_password`).
  - `migrate_component` reads the migration version outside its transaction. Open the stores one after the other, never concurrently; this matters for 3b-ii's listeners.
  - The stored `User-Agent` keeps bidi and zero-width characters. The frontend's session list must render it escaped and isolated.
  - Sessions have no absolute cap, only the 30-day sliding expiry. An option for 3b-ii, for example 90 days from login.
  - `setup-url` has a symlink test but no hard-link test.
  - axum's own 400, 415 and 422 rejections are plain text, not the `ApiError` shape.
  - The setup page lacks `X-Content-Type-Options: nosniff`, and a CSP `default-src`, `connect-src` and `form-action`; add them with the frontend's headers.
  - `rfc3339` is duplicated; the kernel should own it.
  - The route table is an allowlist that a new route can miss. A deny-by-default operator router with an explicit exempt allowlist, or a test that fails on any route outside the two lists, would close it.
  - The step-up limiter is keyed by address; it could also be keyed by session.
  - No test pins a stream ending at its session's expiry (only at a revoke or a logout).
  - `host join`'s stdin `read_line` has no length bound.
- **The setup page's script** is checked only statically: served, and naming `location.hash` and `/api/setup`. A browser test of it comes with the frontend's own test setup.
- **Spec amendments:** decisions 1, 14 and 16, and in kernel §3.3 the rule that every method but `GET` and `HEAD` is state-changing (decision 8).

**Carried from plan 3a, unchanged:**
- `wss://` for remote hosts (3a decision 10), with the `hennery-hello-nonce` header live-checked through Caddy, nginx and `tailscale serve`, and a smaller first-frame limit;
- `PATCH /api/hosts/{id}`;
- `hello.agents`, `workspace_roots` and `probe_agents`;
- a revoked host's sessions in the frontend;
- `doctor`'s re-pair checks;
- the gateway's `LifecycleHooks`;
- `on_hat_purged`;
- `host.lock`;
- the supervisor's restart policy;
- orphaned outboxes;
- 3a's own spec amendments.
- 3a's deferred items:
  - M2 (the exit hook fails open);
  - M4 (the re-pair crash window);
  - `pairing_codes` never pruned;
  - host names' Unicode format characters;
  - `orphan_outbox` without a directory fsync;
  - Task 4's untested paths;
  - the unknown-host-id timing.

  M3 and M5 are closed here (decisions 9, 11 and 12).
- Everything 3a carried from plans (2) and B2b.

Then, in order:
- **(3b-ii) Listeners, `config.toml`, the admin socket and `owner_id`**
- **(3c) Passkeys**
- **(4) Frontend shell**, including the setup page, the login form and the question cards
- **(5) Hats**
- **(6) Gateway**
- **(7) Distribution**

---

_Generated with Claude AI — please review before distribution._
