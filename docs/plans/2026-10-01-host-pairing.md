# Host pairing and identity (plan 3a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** A host proves who it is with its own key instead of the shared development token. The operator mints a single-use pairing code, `hennery host join <url> <code>` generates the host's Ed25519 key and enrolls it, and every `hello` then carries a signature over a nonce the collector picked for that connection. `hennery up` pairs its own host through inherited file descriptors. Revoking a host closes its connection, refuses it for good, parks its sessions and cancels what they held, and the host stops its adapters when it is told.

**Architecture:**
- **Kernel** (`hennery-kernel`): the host registry (`hosts`, `pairing_codes`) in `hennery.db`, migrated as its own component beside the sessions store. Pairing codes are Crockford base32 and hashed at rest. The proof is checked there (`ed25519-dalek`), and there is a failure-counting rate limiter per client address. `LifecycleHooks` is the trait the sessions module implements for a revoke.
- **Wire** (`hennery-proto`): `hello.proof` replaces `hello.token`. The message a host signs is defined once (`hello_proof_message`). The nonce travels in the upgrade response's `hennery-hello-nonce` header. REST gains the pairing and host types and `PendingReason::HostRevoked`.
- **Collector** (`hennery-sessions`):
  - `POST /api/hosts/pairing-codes` (operator);
  - `POST /api/hosts/enroll` (code only, rate limited on the peer address);
  - `GET /api/hosts` (the registry, with `connected`);
  - `DELETE /api/hosts/{id}`: mark revoked, close the connection and wait for it to go, then park the sessions.

  The WebSocket issues a nonce per upgrade, verifies the proof, and re-checks revocation right after registering.
- **Host** (`hennery-host`):
  - `HostKey` and the stored pairing (`host.key`, `host.toml`, mode 0600);
  - `pairing::join`, idempotent through a probe `hello`;
  - `run_until` stops every adapter and returns an error once the collector says `revoked`.
- **Binary:**
  - `hennery host join`;
  - `host run` reads the stored pairing;
  - `hennery up` hands the collector child and the host child the two ends of one pipe, so the code never touches a command line or the environment.

**Tech Stack:** Rust (edition 2024, MSRV 1.88), tokio, axum 0.8 (`ConnectInfo`), tokio-tungstenite 0.29 (`accept_hdr_async` in tests), rusqlite 0.40, reqwest 0.12 (rustls). New crates, pinned exactly in `[workspace.dependencies]`: `ed25519-dalek = "=2.2.0"`, `sha2 = "=0.10.9"`, `getrandom = "=0.3.4"`, `hex = "=0.4.3"`, `toml = "=1.1.6"`. All are pure Rust: the nix dev shell needs nothing new. `std::io::pipe` needs Rust 1.87, under the MSRV. Nix flake dev shell.

**Spec:** [`docs/specs/2026-09-26-kernel-design.md`](../specs/2026-09-26-kernel-design.md). The relevant sections are:
- §1 (one database; every table's `owner_id`, deferred here, decision 2);
- §2 ("Secrets are never accepted as CLI flags");
- §3.3 (enrollment and the host WebSocket are exempt from the browser rules; enrollment is authenticated by its code);
- §3.4 (step-up for minting codes and revoking hosts, deferred to 3b);
- §4.1 (pairing), §4.2 (all-in-one pairing over inherited descriptors), §4.3 (revoke, `last_seen`, one live connection);
- §5.5 (`LifecycleHooks`);
- §11 (pairing tests: expiry, single use, per-address rate limiting without invalidating other codes, idempotent re-join, a revoked host rejected in `hello`).

It also relies on:
- [`docs/specs/2026-09-26-acp-core-design.md`](../specs/2026-09-26-acp-core-design.md):
  - §3.3 (`hello` fields, `hello_error` codes);
  - §3.5 (host authentication);
  - §4.2 (`active/*` → presumed `parked` on revoke);
  - §4.8, §5.1, §5.3 (presumed park keeps questions open; the revoke closes them instead);
  - §8 (`presumed_parked{host_revoked}`).
- The umbrella spec [`2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md): §5.9 (host identity on the wire), §7.5 (TLS except on loopback), §7.6 (pairing), §8.4 (the host key is readable by every agent of that OS user).
- [`2026-09-26-distribution-design.md`](../specs/2026-09-26-distribution-design.md): §1 (`hennery host join <url> <code>`, `hennery host run`), §5.1 (the supervisor's pipe), §8 (`host.key`, `host.toml`).

It builds on the executed [permission and elicitation plan](2026-10-01-permissions.md) (plan 2). Read its "Execution status" and "After this plan" first. Its code wins over its task text, and every anchor below was taken from that code (`feat/permissions` at `c6315b2`, which is about to merge to `main`).

**Status:** not executed. Every code block below was built and tested in a scratch copy of `c6315b2`. Every block was generated from the scratch commits. The plan was then replayed from its own text, task by task, onto a fresh copy of `c6315b2`: each block applied exactly as "Reading the steps" says, and after every task the tree matched the scratch commit byte for byte. After every task the replay ran fmt, clippy (also on the shipped binary with test hooks off), the workspace tests and the codegen check. It ends with 356 tests, up from 309. Every new timing-sensitive test passed with four copies of its test binary running at once.

## Scope

This is **plan (3), real auth and pairing**, as every earlier plan handed it on. It does not fit in one plan of right-sized tasks: operator setup, password login, sessions and cookies, the `Origin` rules, step-up, several listeners, the admin socket and passkeys are a second system of about the same size. It is **split**:
- **3a, this plan (7 tasks):** host identity. Pairing codes, enrollment, `host join`, the `hello` proof, the all-in-one pairing, and revoke. The development token leaves the host path entirely (no `hello.token`, no `--dev-token` on `host run`).
- **3b, next (outlined in "After this plan"):** operator auth. The REST API keeps the development bearer until then, so minting a code and revoking a host are bearer-protected here, **without step-up**.
- **3c, after it:** passkeys. `webauthn-rs` needs OpenSSL, which is a flake change of its own.

**In:**
- Kernel: the host registry and its migration; pairing codes (mint, normalise, hash, expire, spend once); enrollment; the proof check; the rate limiter; `LifecycleHooks`.
- Wire: `hello.proof`, the proof message and nonce header, `PendingReason::HostRevoked`, and the REST types `PairingCodeResponse`, `EnrollRequest`, `EnrollResponse` and `HostItem`.
- Collector: the four host endpoints; the per-connection nonce, proof check and `last_seen` update; the revoke order; the store's revoke of a host's sessions.
- Host: the key and the stored pairing; `join` with its probe and re-pairing; stopping on `revoked`.
- Binary: `host join`, `host run` from the stored pairing, and `up` pairing its own host.

**Out** (later plans; see "After this plan"):
- everything operator-facing (3b, 3c), including step-up on the two endpoints above;
- renaming a host and its default hat (`PATCH /api/hosts/{id}`), which come with hats;
- `wss://` for remote hosts (the WebSocket client has no TLS feature yet, decision 10);
- `hello.agents` and `workspace_roots`, `probe_agents`, `host.lock`;
- the gateway's `SessionMcp::revoke`, the frontend, distribution.

**Where the earlier hand-offs land:**

| Hand-off (plan) | Here |
|---|---|
| "(3) Real auth and pairing", replacing the shared development token (skeleton, A, B1, B2a, B2b, (2)) | Split into 3a/3b/3c. This plan takes it off the host path (Tasks 1–5). REST keeps it until 3b |
| `hello.token` → Ed25519 proof of possession (skeleton) | Task 5 |
| Presumed park on host revoke (B1) | Task 6: `presumed_parked{host_revoked}` |
| Host revoke must cancel open questions, with a new reason `host_revoked` ((2)) | Tasks 6 and 7 |
| The hub's readiness gate: nothing is sent before a connection's reconciliation (A) | Unchanged, and relied on. `HostItem.connected` is that gate. The probe `hello` (Task 7) never sends `resend_complete`, so it never becomes ready. A revoke kicks the connection and waits for it to unregister before it parks anything (Task 6) |
| The all-in-one supervisor's pairing over inherited descriptors (kernel §4.2) | Task 4 |
| `SessionMcp::revoke` on host revoke (A, B1) | Handed on: the gateway plan implements `LifecycleHooks` |
| The rest of (2)'s "After this plan" and its "Still open" | Unchanged, carried in "After this plan" |

## Decisions this plan makes where the spec is silent

Items marked **(amendment)** depart from explicit spec text and should be written back into it.

1. **The split, and what stays on the bearer.** REST keeps `DevToken`/`require_bearer` until 3b. Minting a code and revoking are therefore "operator" routes, with no step-up yet (kernel §3.4 names both). Enrollment is merged **outside** the bearer layer, so no route added later can inherit it by accident; a test pins that.
2. **The kernel's tables share `hennery.db` as their own component.** `db::migrate_component` versions them in a `schema_versions` row, beside the sessions store's `user_version`. It refuses a newer component the way `migrate` refuses a newer database.
   - That makes two write connections to the file. The spec's single writer thread (§1) is still to come, and the busy timeout covers the overlap until then **(amendment: one writer per database comes later)**.
   - There is no `owner_id` yet, on the kernel tables or the session tables. 3b adds the `owners` table and backfills `owner_id` everywhere at once.
   - There is no `default_hat_id`; the hats plan adds it.
3. **The proof message is labelled and length-delimited (amendment).** The host signs `b"hennery hello proof v1"`, then for each of the nonce, the host id and the protocol version its length (u32, big endian) followed by the bytes. Plain `nonce || host_id || protocol_version` would let two different triples produce the same bytes.
   - The nonce is 32 random bytes per upgrade, sent as lowercase hex in the `hennery-hello-nonce` response header **(amendment: name the header)**.
   - Public keys and signatures travel as lowercase hex. A key is stored lowercase, so the same key in another case is recognised as the same key.
   - A fixed vector (seed `[1; 32]`, nonce `[2; 32]`, `host-1`, `1.0`) is checked by both the host's signer and the kernel's verifier.
4. **What a `hello` is told.** An unknown host id gets `bad_proof`, exactly like a wrong signature. The signature is verified before revocation is looked at, so `revoked` is only ever told to the holder of the key. A host whose collector sent no nonce sends no `hello` at all.
5. **Pairing codes.**
   - Eight characters of Crockford base32 (40 random bits), shown `XXXX-XXXX`.
   - Case, dashes and spaces are ignored; `O` is read as `0`, and `I` and `L` as `1`.
   - Stored as SHA-256 hex. Valid while `expires_at > now`, so a code is dead exactly 600 s after minting.
   - Spent in the same transaction that creates the host.
   - An enrollment whose key is paired already is 409 `already_paired`; a malformed one (key, or a name, version or platform outside 1–64 printable characters) is 400 `invalid`. Neither spends the code.
   - Host ids are collector-minted, `host-` plus 16 hex characters.
   - Per kernel §4.1 and distribution §1, the code is a positional argument of `host join`. It is single-use and dies in ten minutes, so §2's rule against secrets as CLI flags is not broken in spirit; `up` never puts it on a command line (decision 11).
6. **Rate limiting counts failures only, per peer address.**
   - Five wrong codes are free within a window that resets ten minutes after the last one.
   - The fifth locks the address out for 60 s, and every further wrong code doubles that, up to an hour.
   - While locked out, even a right code is answered 429 `rate_limited` with `Retry-After`, without being checked (and so without being spent).
   - A success clears the address.
   - Behind a reverse proxy every client shares the proxy's address. `X-Forwarded-For` is forgeable, so it is not trusted, and one flood of wrong codes slows pairing for everyone for a while.
   - The limiter takes `now`, so its tests never sleep. It is the one 3b's login limit reuses.
7. **Times in kernel tables are integer Unix seconds**, passed in by the caller; REST shows RFC 3339.
8. **Endpoints.**
   - `POST /api/hosts/pairing-codes` answers 201 `{code, expires_at}`.
   - `POST /api/hosts/enroll` answers 201 `{host_id}`, or 400/401 `invalid_code`/409/429.
   - `GET /api/hosts` lists every paired host, revoked ones included, as `HostItem`. `connected` means connected **and** reconciled; it replaces today's list of ready host ids.
   - `DELETE /api/hosts/{id}` answers 200 with the revoked `HostItem`, or 404.
   - The handlers live in `hennery-sessions/src/hosts.rs`, beside the hub they need; the registry itself is the kernel's.
9. **The host's files.**
   - `host.key` holds the 32-byte seed as hex. `host.toml` holds `collector` (the WebSocket URL) and `host_id`. Both are written 0600 through a temporary file and a rename, the key first.
   - `join` writes the new key to `host.key.pending` *before* enrolling, so a directory that cannot hold it never costs a code.
   - Half a pairing is an error, never a silent re-pair.
10. **URLs.**
    - `join` accepts `https://`, or `http://` to a loopback address only (umbrella §7.5), with no path. The WebSocket URL is derived from it (`wss`/`ws`, `/api/hosts/ws`).
    - The workspace's `tokio-tungstenite` has no TLS feature, so a `wss://` host cannot connect yet. Enabling and live-testing it behind a TLS terminator is handed on.
    - `up` joins over loopback even when it listens on every interface.
11. **`up` pairs its own host through one pipe (amendment).** The spec has the supervisor receive the code and pass it on. Here the supervisor creates one pipe and gives its write end to the collector child and its read end to the host child, both as descriptor 3. The code passes between the two children only, and the supervisor never reads it.
    - The collector mints the code only after it has migrated and bound its listener.
    - No pipe exists when `host/host.key` is already there.
    - A revoked all-in-one host exits, and `up` exits with it (restart policy is the distribution plan's). It is not re-paired.
12. **`host join` is idempotent through a probe `hello`.** The probe carries nothing attached, never sends `resend_complete`, and closes.
    - `hello_ack` or `already_connected` mean the collector accepts the key: nothing changes and no code is spent. (`already_connected` is only said after the proof was checked.)
    - `revoked` or `bad_proof` mean the pairing is dead. A new key and a new host id replace it, and **the old outbox is deleted (amendment)**: its frames belong to a host id the collector will never hear from again, and would otherwise be resent unacked after every handshake.
    - A pairing with *another* collector URL is refused. Remove the files to move a host.
13. **Revoke, in this order.**
    1. Mark the host revoked. From then on its `hello`s get `revoked`.
    2. Kick its connection and wait, at most 10 s, until the socket task has unregistered it (`Hub::disconnect_and_wait`).
    3. Call `LifecycleHooks::on_host_revoked`. The sessions module's implementation (`Store::revoke_host`) does this:
       - a `starting` session fails `host_revoked`;
       - every `active` or presumed-parked session gets `presumed_parked{reason: host_revoked}`, its open turn ends (`interrupted`, or `turn_not_delivered` if it never started), and its open questions are cancelled `host_revoked`, which gives a queued answer `delivered: false`;
       - a session whose close was requested is closed instead.

    It is idempotent: a session already presumed parked for the revoke is skipped. A repeated `DELETE` re-runs every step, which heals a revoke cut short.
    - The socket task re-checks revocation right after it registers. Its reader's `select!` is `biased` towards shutdown and the kick, so a kicked connection reads no further frame.
    - Those two guard interleavings that no test reproduces deterministically: a revoke landing between the proof check and the registration, and frames already buffered when the kick lands. They are argued in the code. The tests pin the end state (a host that keeps talking after its revoke cannot reattach) and the waiting primitive.
    - `LifecycleHooks` has only `on_host_revoked` for now; `on_hat_purged` comes with hats.
14. **A revoked host stops.** `run_until` returns an error the moment a `hello` is refused `revoked`, after stopping every adapter the way a shutdown does. Other refusals (`bad_proof`, `incompatible`, `already_connected`) keep the reconnect backoff, as today. `HelloRejected` is a typed error, so callers can tell.
15. **An accepted `hello` updates the registry** (kernel §4.3): `host_version`, `capabilities` and `last_seen_at`. The probe announces the same capabilities as the host, so a probe never blanks them.

**Spec drift to reconcile after review:**
- kernel §1: two write connections until the writer thread;
- kernel §4.2 and distribution §5.1: the pipe between the children;
- ACP core §3.5: the labelled, length-delimited proof message and the `hennery-hello-nonce` header;
- kernel §4.1: re-join through a probe, the outbox dropped on re-pair, and a different collector refused;
- kernel §8: the 201/400/401/409/429 shapes of the two pairing endpoints, `HostItem`;
- ACP core §4.6's reason list: `host_revoked`;
- kernel §3.4: step-up on minting and revoking, still to come in 3b.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- `cargo fmt --all --check` (`max_width = 120`), `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo clippy -p hennery --locked -- -D warnings` (test hooks off) pass after every task. So do `cargo test --workspace --locked` and `cargo run -p hennery-proto --bin gen -- --check`.
- A task that changes a `Cargo.toml` runs one `cargo build --workspace` **without** `--locked` first, and commits the updated `Cargo.lock`. New crates are pinned with `=x.y.z` in `[workspace.dependencies]` and taken from there with `.workspace = true`.
- Generated files (`schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`) are regenerated with `cargo run -p hennery-proto --bin gen` whenever a wire type changes. In `codegen.rs`, new root types go in both `add!` lists.
- Pairing codes (kernel §4.1): "8 characters from a base32 alphabet without ambiguous characters, displayed as `XXXX-XXXX`; TTL 10 minutes; single use; stored hashed."
- "Enrollment is rate limited **per client address** (5 wrong codes per 10 minutes, then exponential backoff); wrong codes never invalidate other outstanding codes." (kernel §4.1)
- "`hello.proof` is an Ed25519 signature over `collector_nonce || host_id || protocol_version` with the key generated at pairing; the collector sends the nonce in the WebSocket upgrade response header before the first frame. The connection is unauthenticated until a valid `hello` arrives. The host id is never self-asserted without the proof." (ACP core §3.5; decision 3 refines the concatenation)
- "the host generates an Ed25519 keypair (`host.key` in its data dir, 0600)" (kernel §4.1)
- "**Idempotent:** if `host.key` already exists and the collector accepts it, `host join` does not pair again." (kernel §4.1)
- The all-in-one code passes "over an **inherited file descriptor** … never on a command line or in the environment. If the host's existing key is accepted, nothing is minted. A revoked all-in-one host is not re-paired automatically." (kernel §4.2)
- "Revoking closes the host's connection, rejects future `hello`s with `revoked`, and calls the lifecycle hooks" (kernel §4.3). "A revoked host gets `hello_error{revoked}` and then stops all its adapters" (ACP core §3.5).
- "**One live connection per host.** A second connection for a `host_id` that is already connected is rejected with `hello_error{already_connected}`" (ACP core §3.5).
- "`POST /api/hosts/enroll` | Pairing code | Exempt" (kernel §3.3): enrollment needs no operator credential.
- "v1 requires TLS for anything but `localhost`" (umbrella §7.5).
- The collector sends no request to a host before that connection's post-`resend_complete` reconciliation (ACP core §5.1 step 4).
- No global installs: tooling comes from the flake dev shell.
- Commits follow Conventional Commits (`feat(kernel): …`) and use the repository's own identity (gmail, unsigned). Push the feature branch after every completed task; never push `main`.

## Review Focus

These are the five inputs most likely to bite a real user that the obvious tests would not exercise, most likely first. Each is pinned by the named tests.

1. **A host revoked while it is connected, mid-handshake, or with a question open and its answer already delivered.**
   - Expected: its socket is closed, and its `hello` is refused `revoked` from then on. Its sessions are presumed parked `host_revoked` and never come back `reattached`, even if the host keeps talking after the revoke. The open turn ends, the question is cancelled `host_revoked`, and the answer's verdict is `delivered: false`.
   - The host stops its adapters and exits with an error. A repeated revoke changes nothing more.
   - (Task 6: `a_revoke_closes_the_hosts_connection_and_parks_its_sessions_for_good`, `a_revoke_during_a_handshake_is_not_undone_by_its_reconciliation`, `a_revoked_hosts_sessions_are_parked_for_good_and_what_they_held_is_cancelled`, `disconnect_and_wait_returns_once_the_socket_task_has_let_go`; Task 7: `a_revoked_host_stops_its_adapters_and_exits`.)
   - The two interleavings decision 13 names are argued in code, not reproduced.
2. **`hennery host join` run again: while the host runs, after a revoke, or against another collector.**
   - Expected: while the collector accepts the key, nothing changes and the code is not spent. After a revoke, a new key and a new id replace the pairing and the old outbox is gone. Another collector is refused, with the way out named.
   - (Task 3: `joining_again_leaves_the_pairing_as_it_is_and_spends_no_code`; Task 7: `joining_again_while_the_host_runs_leaves_it_running`, `joining_again_after_a_revoke_pairs_anew_and_drops_the_old_outbox`, `joining_another_collector_while_paired_is_refused`.)
3. **A code typed the way people type codes:** lower case, without the dash, with spaces, `O` for `0`, `I` or `L` for `1`, or one second too late.
   - Expected: every spelling of a live code works, and a code is dead at exactly 600 s.
   - (Task 1: `a_code_is_read_however_it_is_typed`, `a_minted_code_enrolls_one_host_once`, `a_code_expires_after_ten_minutes`.)
4. **Wrong codes from one address.**
   - Expected: four are free. The fifth locks the address out, and then even the right code gets 429 with a `Retry-After` it can wait for, without being spent. Other outstanding codes stay valid, and other addresses are not affected.
   - (Task 1: `wrong_codes_leave_other_outstanding_codes_valid`; Task 2: `the_fifth_failure_locks_out_and_each_further_one_doubles_the_lockout`, `failures_older_than_the_window_are_forgotten`, `wrong_codes_lock_the_address_out_but_leave_valid_codes_unspent`.)
5. **A forged, replayed or unsolicited `hello`:** another key, a proof lifted from another connection, an unknown host id, or a collector that sends no nonce.
   - Expected: `bad_proof`, the host never registered and never listed as connected. A revoked host hears `revoked` only with a valid proof. A host never signs without a nonce.
   - (Task 1: `a_hello_is_accepted_only_with_a_proof_over_its_own_nonce`, `a_revoked_host_is_told_so_only_with_a_valid_proof`; Task 5: `a_hello_signed_by_another_key_is_rejected_without_registering`, `a_proof_is_good_on_its_own_connection_only`, `an_unknown_host_is_refused_like_a_bad_proof`, `a_collector_that_sends_no_nonce_gets_no_hello`.)

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `Cargo.toml`, `crates/*/Cargo.toml`, `Cargo.lock` | The five new crates; `hex` for sessions and testkit; `serde_json` for the binary's tests | 1, 3, 4, 5 |
| `crates/hennery-proto/src/lib.rs` | `HELLO_NONCE_HEADER`, `hello_proof_message` | 1 |
| `crates/hennery-proto/src/frames.rs`, `rest.rs`, `codegen.rs` | `hello.proof`, `PendingReason::HostRevoked`; `PairingCodeResponse`, `EnrollRequest`, `EnrollResponse`, `HostItem` | 2, 5, 6 |
| `crates/hennery-kernel/src/db.rs` | `migrate_component` | 1 |
| `crates/hennery-kernel/src/secret.rs` | `random_bytes`, `sha256_hex`, `unix_now` | 1 |
| `crates/hennery-kernel/src/hosts.rs` | The registry: codes, enrollment, the proof check, revoke, listing | 1 |
| `crates/hennery-kernel/src/ratelimit.rs` | `Policy`, `Limiter` | 2 |
| `crates/hennery-kernel/src/lifecycle.rs` | `LifecycleHooks` | 6 |
| `crates/hennery-sessions/src/lib.rs` | `AppState.hosts`, `.enroll_limiter`; serving with `ConnectInfo`; `LifecycleHooks for AppState` | 2, 6 |
| `crates/hennery-sessions/src/hosts.rs` | The four host endpoints | 2, 5, 6 |
| `crates/hennery-sessions/src/ws.rs` | Nonce, proof check, `record_hello`, the revoke re-check, `biased` | 5, 6 |
| `crates/hennery-sessions/src/hub.rs`, `store.rs` | `disconnect_and_wait`; `revoke_host` | 6 |
| `crates/hennery-host/src/identity.rs` | `HostKey`, `Paired` | 3 |
| `crates/hennery-host/src/pairing.rs` | URLs, `join` | 3, 7 |
| `crates/hennery-host/src/connection.rs`, `outbox.rs` | Signing `hello`; `HelloRejected`, `probe`, stopping on `revoked`; `outbox::FILE` | 5, 7 |
| `crates/hennery/src/main.rs`, `inherit.rs` | `host join`; `host run` from the pairing; `up`'s pipe | 2–5 |
| Tests: `crates/hennery-kernel/tests/hosts.rs`, `crates/hennery-proto/tests/{proof,frames}.rs`, `crates/hennery-sessions/tests/{store,hub}.rs`, `crates/hennery-testkit/tests/{pairing,join,auth,e2e,reconcile,host_connection,ws_ingest_error}.rs`, `crates/hennery/tests/cli.rs` | | all |

All commands run from the repository root inside the dev shell (`nix develop`, or direnv). Work on a feature branch off `main` (e.g. `feat/host-pairing`), once `feat/permissions` has merged. Each task leaves the workspace compiling, clippy-clean and green, and the binary working: `up` keeps connecting its host through every task.

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block (and a final newline).
- "Replace the whole of `path` with:" overwrites the file with the block (and a final newline).
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate; they change no source file. The plan was replayed exactly this way, from its own text, onto `c6315b2`.

---

### Task 1: The host registry in the kernel

**Files:**
- Modify: `Cargo.toml`, `crates/hennery-kernel/Cargo.toml` (the new crates), `Cargo.lock`
- Modify: `crates/hennery-proto/src/lib.rs`, `crates/hennery-kernel/src/db.rs`, `crates/hennery-kernel/src/lib.rs`
- Create: `crates/hennery-kernel/src/secret.rs`, `crates/hennery-kernel/src/hosts.rs`
- Test: `crates/hennery-proto/tests/proof.rs`, `crates/hennery-kernel/tests/hosts.rs`, `crates/hennery-sessions/tests/store.rs`; unit tests in `db.rs` and `secret.rs`

**Interfaces:**
- Consumes: `hennery_kernel::db::{open, open_in_memory, migrate}`, `hennery_proto::frames::Capabilities` as `c6315b2` has them.
- Produces (`hennery_proto`):
  - `const HELLO_NONCE_HEADER: &str = "hennery-hello-nonce"`;
  - `fn hello_proof_message(nonce: &[u8], host_id: &str, protocol_version: &str) -> Vec<u8>`.
- Produces (`hennery_kernel`):
  - `db::migrate_component(conn: &mut Connection, component: &str, migrations: &[&str]) -> Result<()>`;
  - `secret::{random_bytes::<N>() -> [u8; N], sha256_hex(&[u8]) -> String, unix_now() -> i64}`.
- Produces (`hennery_kernel::hosts`):
  - `PAIRING_CODE_TTL_SECS: i64 = 600`;
  - `normalize_code(&str) -> Option<String>`, `parse_public_key(&str) -> Option<VerifyingKey>`, `verify_proof(public_key: &str, nonce: &[u8], host_id: &str, protocol_version: &str, proof: &str) -> bool`;
  - `struct PairingCode { code: String, expires_at: i64 }`;
  - `struct Enrollment { public_key, name, host_version, platform: String }`, with `fn problem(&self) -> Option<String>`;
  - `enum EnrollOutcome { Enrolled { host_id }, InvalidCode, AlreadyPaired { host_id }, Invalid(String) }`;
  - `enum Registered { Created, AlreadyPaired { host_id } }`;
  - `enum HelloCheck { Accepted, Revoked, BadProof }`;
  - `enum Revoke { Revoked, AlreadyRevoked, NotFound }`;
  - `struct HostRecord { id, name, platform, host_version: String, capabilities: Capabilities, created_at: i64, last_seen_at: Option<i64>, revoked_at: Option<i64> }`;
  - `struct Hosts`, with:
    - `open(&Path)`, `open_in_memory()`;
    - `mint_pairing_code(now) -> PairingCode`;
    - `enroll(code, &Enrollment, now) -> EnrollOutcome`;
    - `register(host_id, &Enrollment, now) -> Registered`;
    - `check_hello(host_id, nonce, protocol_version, proof) -> HelloCheck`;
    - `record_hello(host_id, host_version, &Capabilities, now)`;
    - `is_revoked(host_id) -> bool`;
    - `revoke(host_id, now) -> Revoke`;
    - `host(host_id) -> Option<HostRecord>`;
    - `list() -> Vec<HostRecord>`.

    Every one of them returns `anyhow::Result`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-proto/tests/proof.rs`:

```rust
//! The bytes a host signs in `hello` (ACP core §3.5). The same vector is
//! checked by the host that signs it and the collector that verifies it.

use hennery_proto::hello_proof_message;

#[test]
fn the_proof_message_is_labelled_and_length_delimited() {
    let message = hello_proof_message(&[0xab; 4], "host-1", "1.0");
    let mut expected = b"hennery hello proof v1".to_vec();
    expected.extend_from_slice(&[0, 0, 0, 4, 0xab, 0xab, 0xab, 0xab]);
    expected.extend_from_slice(&[0, 0, 0, 6]);
    expected.extend_from_slice(b"host-1");
    expected.extend_from_slice(&[0, 0, 0, 3]);
    expected.extend_from_slice(b"1.0");
    assert_eq!(message, expected);
}

#[test]
fn moving_bytes_between_the_parts_changes_the_message() {
    // Plain concatenation would make these two the same bytes.
    assert_ne!(
        hello_proof_message(b"nonce", "host-1", "1.0"),
        hello_proof_message(b"nonceh", "ost-1", "1.0")
    );
    assert_ne!(
        hello_proof_message(b"n", "host-1", "1.0"),
        hello_proof_message(b"n", "host-11", ".0")
    );
}
```

Create `crates/hennery-kernel/tests/hosts.rs`:

```rust
//! The host registry (kernel spec §4, §11): pairing codes, enrollment, the
//! `hello` proof and revocation.

use ed25519_dalek::{Signer, SigningKey};
use hennery_kernel::hosts::{
    EnrollOutcome, Enrollment, HelloCheck, Hosts, PAIRING_CODE_TTL_SECS, Registered, Revoke, normalize_code,
    verify_proof,
};
use hennery_proto::frames::{Capabilities, Capability};
use hennery_proto::hello_proof_message;

const NOW: i64 = 1_800_000_000;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn enrollment(key: &SigningKey) -> Enrollment {
    Enrollment {
        public_key: hex::encode(key.verifying_key().as_bytes()),
        name: "laptop".into(),
        host_version: "0.0.0".into(),
        platform: "macos-aarch64".into(),
    }
}

fn proof(key: &SigningKey, nonce: &[u8], host_id: &str) -> String {
    hex::encode(key.sign(&hello_proof_message(nonce, host_id, "1.0")).to_bytes())
}

fn enrolled(outcome: EnrollOutcome) -> String {
    match outcome {
        EnrollOutcome::Enrolled { host_id } => host_id,
        other => panic!("expected an enrollment, got {other:?}"),
    }
}

#[test]
fn a_code_is_read_however_it_is_typed() {
    assert_eq!(normalize_code("ABCD-EFGH").as_deref(), Some("ABCDEFGH"));
    assert_eq!(normalize_code(" abcd efgh ").as_deref(), Some("ABCDEFGH"));
    // O is read as 0, I and L as 1: the alphabet has neither.
    assert_eq!(normalize_code("OOIL-2345").as_deref(), Some("00112345"));
    assert_eq!(normalize_code("ABCD-EFG"), None);
    assert_eq!(normalize_code("ABCD-EFGHJ"), None);
    // U is not in the alphabet, and nothing non-ASCII is.
    assert_eq!(normalize_code("ABCD-EFGU"), None);
    assert_eq!(normalize_code("ABCD-EFGÉ"), None);
}

#[test]
fn a_minted_code_enrolls_one_host_once() {
    let hosts = Hosts::open_in_memory().unwrap();
    let code = hosts.mint_pairing_code(NOW).unwrap();
    assert_eq!(code.expires_at, NOW + PAIRING_CODE_TTL_SECS);
    assert_eq!(code.code.len(), 9, "{}", code.code);
    assert_eq!(&code.code[4..5], "-");
    assert!(normalize_code(&code.code).is_some(), "{}", code.code);

    // Typed in lowercase, without the dash: still the same code.
    let typed = code.code.replace('-', "").to_lowercase();
    let host_id = enrolled(hosts.enroll(&typed, &enrollment(&key(1)), NOW + 1).unwrap());
    let listed = hosts.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].id.as_str(), listed[0].name.as_str(), listed[0].created_at),
        (host_id.as_str(), "laptop", NOW + 1)
    );

    // Single use, even for another key.
    assert_eq!(
        hosts.enroll(&code.code, &enrollment(&key(2)), NOW + 2).unwrap(),
        EnrollOutcome::InvalidCode
    );
}

#[test]
fn a_code_expires_after_ten_minutes() {
    let hosts = Hosts::open_in_memory().unwrap();
    let late = hosts.mint_pairing_code(NOW).unwrap();
    assert_eq!(
        hosts
            .enroll(&late.code, &enrollment(&key(1)), NOW + PAIRING_CODE_TTL_SECS)
            .unwrap(),
        EnrollOutcome::InvalidCode
    );
    let in_time = hosts.mint_pairing_code(NOW).unwrap();
    enrolled(
        hosts
            .enroll(&in_time.code, &enrollment(&key(1)), NOW + PAIRING_CODE_TTL_SECS - 1)
            .unwrap(),
    );
}

#[test]
fn wrong_codes_leave_other_outstanding_codes_valid() {
    let hosts = Hosts::open_in_memory().unwrap();
    let first = hosts.mint_pairing_code(NOW).unwrap();
    let second = hosts.mint_pairing_code(NOW).unwrap();
    assert_ne!(first.code, second.code);
    for wrong in ["0000-0000", "ZZZZ-ZZZZ", "not a code"] {
        assert_eq!(
            hosts.enroll(wrong, &enrollment(&key(1)), NOW).unwrap(),
            EnrollOutcome::InvalidCode
        );
    }
    enrolled(hosts.enroll(&first.code, &enrollment(&key(1)), NOW).unwrap());
    enrolled(hosts.enroll(&second.code, &enrollment(&key(2)), NOW).unwrap());
    assert_eq!(hosts.list().unwrap().len(), 2);
}

#[test]
fn a_key_that_is_paired_already_or_a_malformed_enrollment_does_not_use_the_code() {
    let hosts = Hosts::open_in_memory().unwrap();
    let first = hosts.mint_pairing_code(NOW).unwrap();
    let host_id = enrolled(hosts.enroll(&first.code, &enrollment(&key(1)), NOW).unwrap());

    let code = hosts.mint_pairing_code(NOW).unwrap();
    assert_eq!(
        hosts.enroll(&code.code, &enrollment(&key(1)), NOW).unwrap(),
        EnrollOutcome::AlreadyPaired {
            host_id: host_id.clone()
        }
    );
    // The same key in upper case is the same key.
    let mut shouted = enrollment(&key(1));
    shouted.public_key = shouted.public_key.to_uppercase();
    assert_eq!(
        hosts.enroll(&code.code, &shouted, NOW).unwrap(),
        EnrollOutcome::AlreadyPaired { host_id }
    );
    let mut bad_key = enrollment(&key(2));
    bad_key.public_key = "00".repeat(31);
    assert!(matches!(
        hosts.enroll(&code.code, &bad_key, NOW).unwrap(),
        EnrollOutcome::Invalid(why) if why.contains("public_key")
    ));
    let mut no_name = enrollment(&key(2));
    no_name.name = "  ".into();
    assert!(matches!(
        hosts.enroll(&code.code, &no_name, NOW).unwrap(),
        EnrollOutcome::Invalid(why) if why.contains("name")
    ));
    // The code is still good.
    enrolled(hosts.enroll(&code.code, &enrollment(&key(2)), NOW).unwrap());
}

#[test]
fn a_hello_is_accepted_only_with_a_proof_over_its_own_nonce() {
    let hosts = Hosts::open_in_memory().unwrap();
    assert_eq!(
        hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap(),
        Registered::Created
    );
    let nonce = [7u8; 32];
    let good = proof(&key(1), &nonce, "host-1");
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.0", &good).unwrap(),
        HelloCheck::Accepted
    );
    // Replayed on another connection (another nonce).
    assert_eq!(
        hosts.check_hello("host-1", &[8u8; 32], "1.0", &good).unwrap(),
        HelloCheck::BadProof
    );
    // Signed by another key, for another version, or not hex at all.
    let other = proof(&key(2), &nonce, "host-1");
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.0", &other).unwrap(),
        HelloCheck::BadProof
    );
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.1", &good).unwrap(),
        HelloCheck::BadProof
    );
    assert_eq!(
        hosts.check_hello("host-1", &nonce, "1.0", "zz").unwrap(),
        HelloCheck::BadProof
    );
    // An unknown host looks exactly like a bad proof.
    let stranger = proof(&key(1), &nonce, "host-2");
    assert_eq!(
        hosts.check_hello("host-2", &nonce, "1.0", &stranger).unwrap(),
        HelloCheck::BadProof
    );
}

#[test]
fn a_revoked_host_is_told_so_only_with_a_valid_proof() {
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    assert_eq!(hosts.revoke("host-2", NOW).unwrap(), Revoke::NotFound);
    assert!(!hosts.is_revoked("host-1").unwrap());
    assert_eq!(hosts.revoke("host-1", NOW + 5).unwrap(), Revoke::Revoked);
    assert_eq!(hosts.revoke("host-1", NOW + 6).unwrap(), Revoke::AlreadyRevoked);
    assert!(hosts.is_revoked("host-1").unwrap());
    assert_eq!(hosts.host("host-1").unwrap().unwrap().revoked_at, Some(NOW + 5));

    let nonce = [7u8; 32];
    assert_eq!(
        hosts
            .check_hello("host-1", &nonce, "1.0", &proof(&key(1), &nonce, "host-1"))
            .unwrap(),
        HelloCheck::Revoked
    );
    assert_eq!(
        hosts
            .check_hello("host-1", &nonce, "1.0", &proof(&key(2), &nonce, "host-1"))
            .unwrap(),
        HelloCheck::BadProof
    );
}

#[test]
fn an_accepted_hello_updates_the_hosts_version_capabilities_and_last_seen() {
    let hosts = Hosts::open_in_memory().unwrap();
    hosts.register("host-1", &enrollment(&key(1)), NOW).unwrap();
    hosts
        .record_hello("host-1", "0.1.0", &Capabilities(vec![Capability::Park]), NOW + 9)
        .unwrap();
    let record = hosts.host("host-1").unwrap().unwrap();
    assert_eq!(
        (record.host_version.as_str(), record.capabilities, record.last_seen_at),
        ("0.1.0", Capabilities(vec![Capability::Park]), Some(NOW + 9))
    );
}

/// The vector `hennery-host`'s signer is checked against too: a fixed key,
/// nonce and host id give this exact signature (Ed25519 is deterministic).
const VECTOR_SIGNATURE: &str = "bd2b7388413c333e9ed69c330b4a8be8ffb6228609979b30607236fcdefab259\
cdf6b48fb39bfaa9b5a3cd01538280ec9e6d50c8831e9aae4d791f68112a6c04";

#[test]
fn the_fixed_proof_vector_verifies() {
    let key = key(1);
    let nonce = [2u8; 32];
    let signature = proof(&key, &nonce, "host-1");
    assert_eq!(signature, VECTOR_SIGNATURE);
    assert!(verify_proof(
        &hex::encode(key.verifying_key().as_bytes()),
        &nonce,
        "host-1",
        "1.0",
        VECTOR_SIGNATURE
    ));
}
```

Append to `crates/hennery-sessions/tests/store.rs`:

```rust
// Plan 3a: the kernel's tables share `hennery.db` with this store.

#[test]
fn the_session_store_and_the_host_registry_share_one_database_in_either_order() {
    use hennery_kernel::hosts::Hosts;
    for kernel_first in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let (store, hosts) = if kernel_first {
            let hosts = Hosts::open(&db).unwrap();
            (Store::open(&db).unwrap(), hosts)
        } else {
            let store = Store::open(&db).unwrap();
            (store, Hosts::open(&db).unwrap())
        };
        started(&store);
        hosts.mint_pairing_code(0).unwrap();
        drop((store, hosts));
        // Reopened, each finds its own tables and migrates nothing twice.
        let store = Store::open(&db).unwrap();
        let hosts = Hosts::open(&db).unwrap();
        assert_eq!(store.session("s1").unwrap().unwrap().lifecycle, "active");
        assert!(hosts.list().unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-kernel --test hosts`
Expected: FAIL to compile. The test file names `hennery_kernel::hosts`, and the crates `ed25519_dalek`, `hex` and `hennery_proto`, none of which the kernel has yet: `error[E0432]: unresolved import hennery_kernel::hosts`, `error[E0432]: unresolved import ed25519_dalek`, `error[E0433]: cannot find module or crate hex in this scope`.

- [ ] **Step 3: Add the crates, the proof message and the registry**

Replace the whole of `Cargo.toml` with:

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.0.0"
edition = "2024"
license = "AGPL-3.0-only"
rust-version = "1.88"
publish = false

[workspace.dependencies]
agent-client-protocol = "=2.2.0"
anyhow = "1"
axum = { version = "0.8.9", features = ["ws"] }
clap = { version = "4", features = ["derive", "env"] }
ed25519-dalek = "=2.2.0"
futures = "0.3"
getrandom = "=0.3.4"
hex = "=0.4.3"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
rusqlite = { version = "0.40", features = ["bundled"] }
schemars = "1.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "=0.10.9"
thiserror = "2"
time = { version = "0.3", features = ["formatting"] }
tokio = { version = "1", features = ["full"] }
tokio-stream = { version = "0.1", features = ["sync"] }
tokio-tungstenite = "0.29"
tokio-util = { version = "0.7", features = ["compat"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
ts-rs = { version = "12", features = ["serde-json-impl"] }
uuid = { version = "1", features = ["v7", "serde"] }
hennery-proto = { path = "crates/hennery-proto" }
hennery-kernel = { path = "crates/hennery-kernel" }
hennery-sessions = { path = "crates/hennery-sessions" }
hennery-host = { path = "crates/hennery-host" }
```

Replace the whole of `crates/hennery-kernel/Cargo.toml` with:

```toml
[package]
name = "hennery-kernel"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
anyhow.workspace = true
axum.workspace = true
ed25519-dalek.workspace = true
getrandom.workspace = true
hennery-proto.workspace = true
hex.workspace = true
rusqlite.workspace = true
serde_json.workspace = true
sha2.workspace = true

[dev-dependencies]
tempfile = "3"
```

In `crates/hennery-proto/src/lib.rs`, replace:

```rust
}
```

with:

```rust
}

/// Name of the WebSocket upgrade response header that carries the
/// collector's nonce for `hello.proof` (ACP core §3.5): 32 random bytes,
/// lowercase hex.
pub const HELLO_NONCE_HEADER: &str = "hennery-hello-nonce";

/// The bytes a host signs for `hello.proof` (ACP core §3.5): the
/// collector's nonce, the host id and the protocol version, behind a
/// domain-separation label and each prefixed with its length (u32, big
/// endian), so no two different triples produce the same message.
pub fn hello_proof_message(nonce: &[u8], host_id: &str, protocol_version: &str) -> Vec<u8> {
    const LABEL: &[u8] = b"hennery hello proof v1";
    let mut out = Vec::with_capacity(LABEL.len() + 12 + nonce.len() + host_id.len() + protocol_version.len());
    out.extend_from_slice(LABEL);
    for part in [nonce, host_id.as_bytes(), protocol_version.as_bytes()] {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}
```

In `crates/hennery-kernel/src/db.rs`, replace:

```rust
use rusqlite::Connection;
```

with:

```rust
use rusqlite::{Connection, OptionalExtension};
```

In `crates/hennery-kernel/src/db.rs`, replace:

```rust
}

#[cfg(test)]
```

with:

```rust
}

/// Like `migrate`, for one component of a database that several own
/// (kernel spec §1): the kernel's tables share `hennery.db` with the
/// sessions module, which keeps `user_version` for itself. Each component's
/// version is a row of `schema_versions`, and a component newer than this
/// binary is refused the same way.
pub fn migrate_component(conn: &mut Connection, component: &str, migrations: &[&str]) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_versions (component TEXT PRIMARY KEY, version INTEGER NOT NULL);",
    )?;
    let current: usize = conn
        .query_row(
            "SELECT version FROM schema_versions WHERE component = ?1",
            [component],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0) as usize;
    if current > migrations.len() {
        bail!(
            "{component} schema version {current} is newer than this binary supports ({})",
            migrations.len()
        );
    }
    for (index, sql) in migrations.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_versions(component, version) VALUES (?1, ?2)
             ON CONFLICT(component) DO UPDATE SET version = excluded.version",
            rusqlite::params![component, (index + 1) as i64],
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
```

In `crates/hennery-kernel/src/db.rs`, replace:

```rust
    }
}
```

with:

```rust
    }

    #[test]
    fn components_keep_their_own_versions_beside_user_version() {
        let mut conn = open_in_memory().unwrap();
        migrate(&mut conn, &["CREATE TABLE a (x INTEGER);"]).unwrap();
        migrate_component(
            &mut conn,
            "kernel",
            &["CREATE TABLE k1 (x INTEGER);", "CREATE TABLE k2 (x INTEGER);"],
        )
        .unwrap();
        migrate_component(
            &mut conn,
            "kernel",
            &["CREATE TABLE k1 (x INTEGER);", "CREATE TABLE k2 (x INTEGER);"],
        )
        .unwrap();
        migrate_component(&mut conn, "other", &["CREATE TABLE o1 (x INTEGER);"]).unwrap();
        let user_version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        let kernel: i64 = conn
            .query_row(
                "SELECT version FROM schema_versions WHERE component = 'kernel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((user_version, kernel), (1, 2));
        // `migrate` still sees its own version, untouched by the components.
        migrate(&mut conn, &["CREATE TABLE a (x INTEGER);"]).unwrap();
    }

    #[test]
    fn a_newer_component_is_refused() {
        let mut conn = open_in_memory().unwrap();
        migrate_component(
            &mut conn,
            "kernel",
            &["CREATE TABLE k1 (x INTEGER);", "CREATE TABLE k2 (x INTEGER);"],
        )
        .unwrap();
        let err = migrate_component(&mut conn, "kernel", &["CREATE TABLE k1 (x INTEGER);"]).unwrap_err();
        assert!(err.to_string().contains("kernel schema version 2 is newer"), "{err}");
    }
}
```

Replace the whole of `crates/hennery-kernel/src/lib.rs` with:

```rust
//! Shared collector foundations (kernel spec): storage, request auth, and
//! host identity and pairing.

pub mod auth;
pub mod db;
pub mod hosts;
pub mod secret;
```

Create `crates/hennery-kernel/src/secret.rs`:

```rust
//! Randomness and hashing for credentials (kernel spec §3, §4).

use sha2::{Digest, Sha256};

/// `N` bytes from the operating system's CSPRNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("the OS random number generator is available");
    out
}

/// Lowercase hex SHA-256: how pairing codes (and later session ids) are
/// stored at rest.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Seconds since the Unix epoch, the kernel tables' time unit.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_differ_between_calls() {
        assert_ne!(random_bytes::<32>(), random_bytes::<32>());
    }

    #[test]
    fn sha256_hex_matches_the_known_digest_of_abc() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
```

Create `crates/hennery-kernel/src/hosts.rs`:

```rust
//! Host identity and pairing (kernel spec §4): the `hosts` registry, one-time
//! pairing codes, and the check of a host's `hello` proof (ACP core §3.5).
//!
//! Every time is seconds since the Unix epoch and is passed in by the
//! caller (`secret::unix_now()` in production), so expiry is testable
//! without sleeping.

use crate::db;
use crate::secret::{random_bytes, sha256_hex};
use anyhow::Result;
use ed25519_dalek::{Signature, VerifyingKey};
use hennery_proto::frames::Capabilities;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
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

/// A pairing code is valid this long (kernel spec §4.1).
pub const PAIRING_CODE_TTL_SECS: i64 = 10 * 60;

/// Crockford's base32 alphabet: no `I`, `L`, `O` or `U`.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Longest accepted host name, version and platform string.
const MAX_FIELD: usize = 64;

/// A freshly minted pairing code, shown once.
#[derive(Debug, Clone, PartialEq)]
pub struct PairingCode {
    /// `XXXX-XXXX`.
    pub code: String,
    pub expires_at: i64,
}

/// What a host sends to enroll (kernel spec §4.1), besides the code.
#[derive(Debug, Clone, PartialEq)]
pub struct Enrollment {
    /// The host's Ed25519 public key, 64 lowercase hex characters.
    pub public_key: String,
    pub name: String,
    pub host_version: String,
    pub platform: String,
}

impl Enrollment {
    /// Why this enrollment cannot be stored, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if parse_public_key(&self.public_key).is_none() {
            return Some("public_key must be an Ed25519 public key in hex".into());
        }
        for (field, value) in [
            ("name", &self.name),
            ("host_version", &self.host_version),
            ("platform", &self.platform),
        ] {
            let value = value.trim();
            if value.is_empty() || value.chars().count() > MAX_FIELD || value.chars().any(char::is_control) {
                return Some(format!("{field} must be 1 to {MAX_FIELD} printable characters"));
            }
        }
        None
    }
}

/// The outcome of `Hosts::enroll`.
#[derive(Debug, Clone, PartialEq)]
pub enum EnrollOutcome {
    Enrolled {
        host_id: String,
    },
    /// Unknown, expired or already used: one answer for all three.
    InvalidCode,
    /// A host with this public key is paired already; the code is not used.
    AlreadyPaired {
        host_id: String,
    },
    /// The enrollment itself is malformed (why); the code is not used.
    Invalid(String),
}

/// The outcome of `Hosts::register`.
#[derive(Debug, Clone, PartialEq)]
pub enum Registered {
    Created,
    AlreadyPaired { host_id: String },
}

/// The verdict on a `hello` (ACP core §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloCheck {
    Accepted,
    /// The proof is valid, and the host is revoked.
    Revoked,
    /// Unknown host, or a proof that does not verify.
    BadProof,
}

/// The outcome of `Hosts::revoke`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revoke {
    Revoked,
    AlreadyRevoked,
    NotFound,
}

/// One paired host, as `GET /api/hosts` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostRecord {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    /// From its latest accepted `hello`.
    pub capabilities: Capabilities,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// A pairing code as typed, reduced to its eight canonical characters:
/// case, dashes and spaces ignored, `O` read as `0`, `I` and `L` as `1`.
pub fn normalize_code(input: &str) -> Option<String> {
    let mut out = String::with_capacity(8);
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == 8).then_some(out)
}

/// A new random code, `XXXX-XXXX` (40 bits).
fn new_code() -> String {
    let bits = u64::from_be_bytes({
        let mut b = [0u8; 8];
        b[3..].copy_from_slice(&random_bytes::<5>());
        b
    });
    let mut out = String::with_capacity(9);
    for i in 0..8 {
        if i == 4 {
            out.push('-');
        }
        let index = (bits >> (35 - 5 * i)) & 0x1f;
        out.push(ALPHABET[index as usize] as char);
    }
    out
}

fn code_hash(normalized: &str) -> String {
    sha256_hex(normalized.as_bytes())
}

/// A hex Ed25519 public key that is a valid point and not of small order.
pub fn parse_public_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_key).ok()?.try_into().ok()?;
    let key = VerifyingKey::from_bytes(&bytes).ok()?;
    (!key.is_weak()).then_some(key)
}

/// Whether `proof` (hex) is `public_key`'s signature over the hello message
/// for this nonce, host id and protocol version.
pub fn verify_proof(public_key: &str, nonce: &[u8], host_id: &str, protocol_version: &str, proof: &str) -> bool {
    let Some(key) = parse_public_key(public_key) else {
        return false;
    };
    let Some(signature) = hex::decode(proof)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .map(|b| Signature::from_bytes(&b))
    else {
        return false;
    };
    let message = hennery_proto::hello_proof_message(nonce, host_id, protocol_version);
    key.verify_strict(&message, &signature).is_ok()
}

pub struct Hosts {
    conn: Mutex<Connection>,
}

impl Hosts {
    /// Open the kernel's tables in `hennery.db` (shared with the sessions
    /// store; each migrates only its own tables).
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("hosts lock")
    }

    /// Mint a single-use pairing code valid for `PAIRING_CODE_TTL_SECS`.
    /// Only its hash is stored.
    pub fn mint_pairing_code(&self, now: i64) -> Result<PairingCode> {
        let code = new_code();
        let expires_at = now + PAIRING_CODE_TTL_SECS;
        let normalized = normalize_code(&code).expect("a minted code is canonical");
        self.conn().execute(
            "INSERT INTO pairing_codes(code_hash, created_at, expires_at) VALUES (?1, ?2, ?3)",
            params![code_hash(&normalized), now, expires_at],
        )?;
        Ok(PairingCode { code, expires_at })
    }

    /// Pair a host with a code (kernel spec §4.1): check the code, create the
    /// host with a fresh id, and use the code up, in one transaction. A
    /// wrong code changes nothing, so other outstanding codes stay valid.
    pub fn enroll(&self, code: &str, enrollment: &Enrollment, now: i64) -> Result<EnrollOutcome> {
        if let Some(problem) = enrollment.problem() {
            return Ok(EnrollOutcome::Invalid(problem));
        }
        let Some(normalized) = normalize_code(code) else {
            return Ok(EnrollOutcome::InvalidCode);
        };
        let hash = code_hash(&normalized);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let usable = tx
            .query_row(
                "SELECT 1 FROM pairing_codes WHERE code_hash = ?1 AND used_at IS NULL AND expires_at > ?2",
                params![hash, now],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !usable {
            return Ok(EnrollOutcome::InvalidCode);
        }
        let host_id = new_host_id();
        match insert_host(&tx, &host_id, enrollment, now)? {
            Registered::Created => {}
            Registered::AlreadyPaired { host_id } => return Ok(EnrollOutcome::AlreadyPaired { host_id }),
        }
        tx.execute(
            "UPDATE pairing_codes SET used_at = ?2 WHERE code_hash = ?1",
            params![hash, now],
        )?;
        tx.commit()?;
        Ok(EnrollOutcome::Enrolled { host_id })
    }

    /// Store a host under a known id, without a code: what `enroll` does once
    /// the code checks out. Also how tests pair a host under a fixed id.
    pub fn register(&self, host_id: &str, enrollment: &Enrollment, now: i64) -> Result<Registered> {
        anyhow::ensure!(
            enrollment.problem().is_none(),
            "invalid enrollment: {:?}",
            enrollment.problem()
        );
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let registered = insert_host(&tx, host_id, enrollment, now)?;
        tx.commit()?;
        Ok(registered)
    }

    /// Check a `hello` (ACP core §3.5). The proof is verified before the
    /// revocation is looked at, so `revoked` is only ever told to the
    /// holder of the key.
    pub fn check_hello(&self, host_id: &str, nonce: &[u8], protocol_version: &str, proof: &str) -> Result<HelloCheck> {
        let row: Option<(String, Option<i64>)> = self
            .conn()
            .query_row(
                "SELECT public_key, revoked_at FROM hosts WHERE id = ?1",
                [host_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((public_key, revoked_at)) = row else {
            return Ok(HelloCheck::BadProof);
        };
        if !verify_proof(&public_key, nonce, host_id, protocol_version, proof) {
            return Ok(HelloCheck::BadProof);
        }
        Ok(if revoked_at.is_some() {
            HelloCheck::Revoked
        } else {
            HelloCheck::Accepted
        })
    }

    /// Update what an accepted `hello` reports (kernel spec §4.3).
    pub fn record_hello(&self, host_id: &str, host_version: &str, capabilities: &Capabilities, now: i64) -> Result<()> {
        self.conn().execute(
            "UPDATE hosts SET host_version = ?2, capabilities = ?3, last_seen_at = ?4 WHERE id = ?1",
            params![host_id, host_version, serde_json::to_string(capabilities)?, now],
        )?;
        Ok(())
    }

    pub fn is_revoked(&self, host_id: &str) -> Result<bool> {
        let revoked: Option<Option<i64>> = self
            .conn()
            .query_row("SELECT revoked_at FROM hosts WHERE id = ?1", [host_id], |r| r.get(0))
            .optional()?;
        Ok(matches!(revoked, Some(Some(_))))
    }

    /// Mark a host revoked (kernel spec §4.3). Its future `hello`s get
    /// `revoked`; closing its connection and parking its sessions is the
    /// caller's part.
    pub fn revoke(&self, host_id: &str, now: i64) -> Result<Revoke> {
        let conn = self.conn();
        let revoked: Option<Option<i64>> = conn
            .query_row("SELECT revoked_at FROM hosts WHERE id = ?1", [host_id], |r| r.get(0))
            .optional()?;
        Ok(match revoked {
            None => Revoke::NotFound,
            Some(Some(_)) => Revoke::AlreadyRevoked,
            Some(None) => {
                conn.execute("UPDATE hosts SET revoked_at = ?2 WHERE id = ?1", params![host_id, now])?;
                Revoke::Revoked
            }
        })
    }

    pub fn host(&self, host_id: &str) -> Result<Option<HostRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {HOST_COLUMNS} FROM hosts WHERE id = ?1"))?;
        Ok(stmt.query_row([host_id], read_host).optional()?)
    }

    /// Every paired host, revoked ones included, oldest first.
    pub fn list(&self) -> Result<Vec<HostRecord>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!("SELECT {HOST_COLUMNS} FROM hosts ORDER BY created_at, id"))?;
        let rows = stmt.query_map([], read_host)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

const HOST_COLUMNS: &str = "id, name, platform, host_version, capabilities, created_at, last_seen_at, revoked_at";

fn read_host(r: &rusqlite::Row<'_>) -> rusqlite::Result<HostRecord> {
    let capabilities: String = r.get(4)?;
    Ok(HostRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        platform: r.get(2)?,
        host_version: r.get(3)?,
        capabilities: serde_json::from_str(&capabilities).unwrap_or_default(),
        created_at: r.get(5)?,
        last_seen_at: r.get(6)?,
        revoked_at: r.get(7)?,
    })
}

fn new_host_id() -> String {
    format!("host-{}", hex::encode(random_bytes::<8>()))
}

fn insert_host(tx: &rusqlite::Transaction<'_>, host_id: &str, e: &Enrollment, now: i64) -> Result<Registered> {
    // Stored lowercase, so the same key in another case is the same key.
    let public_key = e.public_key.to_ascii_lowercase();
    let existing: Option<String> = tx
        .query_row("SELECT id FROM hosts WHERE public_key = ?1", [&public_key], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(host_id) = existing {
        return Ok(Registered::AlreadyPaired { host_id });
    }
    tx.execute(
        "INSERT INTO hosts(id, name, public_key, platform, host_version, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            host_id,
            e.name.trim(),
            public_key,
            e.platform.trim(),
            e.host_version.trim(),
            now
        ],
    )?;
    Ok(Registered::Created)
}
```

- [ ] **Step 4: Update the lock file and run the new tests**

Run: `cargo build --workspace` (no `--locked`: it records the five new crates in `Cargo.lock`), then `cargo test -p hennery-kernel -p hennery-proto --locked` and `cargo test -p hennery-sessions --test store the_session_store_and_the_host_registry --locked`.
Expected: all pass, among them `the_fixed_proof_vector_verifies` and `components_keep_their_own_versions_beside_user_version`.

- [ ] **Step 5: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 325 tests pass; nothing to regenerate.

- [ ] **Step 6: Commit and push**

```bash
git add Cargo.toml Cargo.lock crates/hennery-proto crates/hennery-kernel crates/hennery-sessions/tests/store.rs
git commit -m "feat(kernel): host registry, pairing codes and the hello proof check"
git push
```

### Task 2: Pairing codes and enrollment over HTTP

**Files:**
- Create: `crates/hennery-kernel/src/ratelimit.rs`, `crates/hennery-sessions/src/hosts.rs`
- Modify: `crates/hennery-kernel/src/lib.rs`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, `crates/hennery-sessions/src/lib.rs`, `crates/hennery-sessions/src/api.rs`, `crates/hennery/src/main.rs`
- Modify (`AppState::new` gains the registry): `crates/hennery-testkit/tests/{auth,e2e,reconcile,ws_ingest_error}.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-testkit/tests/pairing.rs`; unit tests in `ratelimit.rs`

**Interfaces:**
- Consumes: Task 1's `Hosts`, `Enrollment`, `EnrollOutcome`, `secret::unix_now`.
- Produces (`hennery_kernel::ratelimit`):
  - `struct Policy { free_failures: u32, window, first_lockout, max_lockout: Duration }`, with `Policy::ENROLL`;
  - `struct Limiter`, with `new(Policy)`, `check(IpAddr, Instant) -> Result<(), Duration>` (the `Err` is the retry-after), `failed(IpAddr, Instant)` and `succeeded(IpAddr)`.
- Produces (`hennery_proto::rest`): `PairingCodeResponse { code, expires_at: String }`, `EnrollRequest { code, public_key, name, host_version, platform: String }`, `EnrollResponse { host_id: String }`.
- Produces (`hennery_sessions`):
  - `AppState::new(store: Store, hosts: Hosts, token: DevToken)`, with the new fields `hosts: Arc<Hosts>` and `enroll_limiter: Arc<Limiter>`;
  - `hosts::router(AppState) -> Router`;
  - `pub(crate) hosts::rfc3339(i64) -> String`;
  - `pub(crate) api::{error, internal}`.

  `serve` (and the binary) serve with `into_make_service_with_connect_info::<SocketAddr>()`.
- Produces (HTTP):
  - `POST /api/hosts/pairing-codes` (bearer): 201 `PairingCodeResponse`;
  - `POST /api/hosts/enroll` (no bearer): 201 `EnrollResponse`, 400 `invalid`, 401 `invalid_code`, 409 `already_paired`, or 429 `rate_limited` with `Retry-After`.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-testkit/tests/pairing.rs`:

```rust
//! Host pairing over HTTP (kernel spec §4.1, §11): minting a code needs the
//! operator, enrollment needs only the code, and wrong codes lock the
//! client's address out without spoiling the codes that are still valid.

use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, Hosts};
use hennery_proto::rest::{ApiError, EnrollRequest, EnrollResponse, PairingCodeResponse};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const TOKEN: &str = "dev-token";
/// Valid Ed25519 public keys (RFC 8032 §7.1, tests 1 to 3).
const KEYS: [&str; 3] = [
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
    "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
];

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            DevToken::new(TOKEN),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn mint(&self) -> String {
        let resp = reqwest::Client::new()
            .post(self.url("/api/hosts/pairing-codes"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        resp.json::<PairingCodeResponse>().await.unwrap().code
    }

    async fn enroll(&self, code: &str, key: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(self.url("/api/hosts/enroll"))
            .json(&EnrollRequest {
                code: code.into(),
                public_key: key.into(),
                name: "laptop".into(),
                host_version: "0.0.0".into(),
                platform: "linux-x86_64".into(),
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

#[tokio::test]
async fn a_minted_code_pairs_a_host_without_the_operator_bearer_once() {
    let collector = Collector::start().await;
    let code = collector.mint().await;
    let resp = collector.enroll(&code, KEYS[0]).await;
    assert_eq!(resp.status(), 201);
    let host_id = resp.json::<EnrollResponse>().await.unwrap().host_id;
    let record = collector.state.hosts.host(&host_id).unwrap().expect("host stored");
    assert_eq!(
        (record.name.as_str(), record.platform.as_str()),
        ("laptop", "linux-x86_64")
    );

    assert_eq!(
        code_of(collector.enroll(&code, KEYS[1]).await).await,
        (401, "invalid_code".into())
    );
}

#[tokio::test]
async fn minting_needs_the_operator_and_enrollment_does_not() {
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
        code_of(collector.enroll("0000-0000", KEYS[0]).await).await,
        (401, "invalid_code".into())
    );
}

#[tokio::test]
async fn wrong_codes_lock_the_address_out_but_leave_valid_codes_unspent() {
    let collector = Collector::start().await;
    let code = collector.mint().await;
    for _ in 0..4 {
        assert_eq!(
            code_of(collector.enroll("0000-0000", KEYS[0]).await).await,
            (401, "invalid_code".into())
        );
    }
    // Four wrong codes are free, and a right one clears them.
    assert_eq!(collector.enroll(&code, KEYS[0]).await.status(), 201);

    let spare = collector.mint().await;
    for _ in 0..5 {
        assert_eq!(collector.enroll("0000-0000", KEYS[1]).await.status(), 401);
    }
    // Locked out: even the right code is not looked at.
    let locked = collector.enroll(&spare, KEYS[1]).await;
    assert_eq!(locked.status(), 429);
    let retry_after: u64 = locked.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(locked.json::<ApiError>().await.unwrap().code, "rate_limited");
    // The spare code was never spent: it still pairs a host.
    let enrollment = Enrollment {
        public_key: KEYS[1].into(),
        name: "desk".into(),
        host_version: "0.0.0".into(),
        platform: "linux-x86_64".into(),
    };
    assert!(matches!(
        collector
            .state
            .hosts
            .enroll(&spare, &enrollment, hennery_kernel::secret::unix_now())
            .unwrap(),
        EnrollOutcome::Enrolled { .. }
    ));
}

#[tokio::test]
async fn a_malformed_or_already_paired_enrollment_is_refused_and_keeps_the_code() {
    let collector = Collector::start().await;
    let first = collector.mint().await;
    assert_eq!(collector.enroll(&first, KEYS[0]).await.status(), 201);

    let code = collector.mint().await;
    assert_eq!(
        code_of(collector.enroll(&code, "not-a-key").await).await,
        (400, "invalid".into())
    );
    assert_eq!(
        code_of(collector.enroll(&code, KEYS[0]).await).await,
        (409, "already_paired".into())
    );
    assert_eq!(collector.enroll(&code, KEYS[2]).await.status(), 201);
    assert_eq!(collector.state.hosts.list().unwrap().len(), 2);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --test pairing`
Expected: FAIL to compile. `error[E0432]: unresolved imports hennery_proto::rest::EnrollRequest, hennery_proto::rest::EnrollResponse, hennery_proto::rest::PairingCodeResponse`, `error[E0061]: this function takes 2 arguments but 3 arguments were supplied` (`AppState::new`) and `error[E0609]: no field hosts on type AppState`.

- [ ] **Step 3: The limiter, the REST types and the endpoints**

The limiter's own tests are in the module. `AppState::new` now takes the registry, so each existing harness opens one on its database file. `ws_ingest_error` uses one in memory: those tests lock the session database on purpose.

Create `crates/hennery-kernel/src/ratelimit.rs`:

```rust
//! Failure-based rate limiting per client address (kernel spec §3.2, §4.1):
//! a few free failures within a window, then an exponentially growing
//! lockout. Only failures count; while an address is locked out its
//! attempts are refused without being checked.
//!
//! Callers pass `now`, so the policy is testable without sleeping.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Failures allowed within `window` before the first lockout.
    pub free_failures: u32,
    /// Failures older than this (and no lockout pending) are forgotten.
    pub window: Duration,
    /// The first lockout; each further failure doubles it.
    pub first_lockout: Duration,
    pub max_lockout: Duration,
}

impl Policy {
    /// Host enrollment: 5 wrong codes per 10 minutes (kernel spec §4.1).
    pub const ENROLL: Policy = Policy {
        free_failures: 5,
        window: Duration::from_secs(10 * 60),
        first_lockout: Duration::from_secs(60),
        max_lockout: Duration::from_secs(60 * 60),
    };
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    failures: u32,
    last_failure: Instant,
    locked_until: Option<Instant>,
}

impl Entry {
    fn forgotten(&self, policy: &Policy, now: Instant) -> bool {
        let locked = self.locked_until.is_some_and(|until| now < until);
        !locked && now.saturating_duration_since(self.last_failure) >= policy.window
    }
}

/// Entries kept before stale ones are pruned.
const PRUNE_ABOVE: usize = 4096;

pub struct Limiter {
    policy: Policy,
    entries: Mutex<HashMap<IpAddr, Entry>>,
}

impl Limiter {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// `Err(retry_after)` while `addr` is locked out.
    pub fn check(&self, addr: IpAddr, now: Instant) -> Result<(), Duration> {
        let entries = self.entries.lock().expect("limiter lock");
        match entries.get(&addr).and_then(|e| e.locked_until) {
            Some(until) if now < until => Err(until - now),
            _ => Ok(()),
        }
    }

    /// Record a failed attempt from `addr`.
    pub fn failed(&self, addr: IpAddr, now: Instant) {
        let policy = self.policy;
        let mut entries = self.entries.lock().expect("limiter lock");
        if entries.len() > PRUNE_ABOVE {
            entries.retain(|_, e| !e.forgotten(&policy, now));
        }
        let entry = entries.entry(addr).or_insert(Entry {
            failures: 0,
            last_failure: now,
            locked_until: None,
        });
        if entry.forgotten(&policy, now) {
            entry.failures = 0;
            entry.locked_until = None;
        }
        entry.failures += 1;
        entry.last_failure = now;
        if entry.failures >= policy.free_failures {
            let doublings = (entry.failures - policy.free_failures).min(20);
            let lockout = policy
                .first_lockout
                .saturating_mul(1 << doublings)
                .min(policy.max_lockout);
            entry.locked_until = Some(now + lockout);
        }
    }

    /// A successful attempt forgets `addr`'s failures.
    pub fn succeeded(&self, addr: IpAddr) {
        self.entries.lock().expect("limiter lock").remove(&addr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1));
    const B: IpAddr = IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 2));

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn the_fifth_failure_locks_out_and_each_further_one_doubles_the_lockout() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        for _ in 0..4 {
            limiter.failed(A, t0);
            assert_eq!(limiter.check(A, t0), Ok(()));
        }
        limiter.failed(A, t0);
        assert_eq!(limiter.check(A, t0), Err(secs(60)));
        assert_eq!(limiter.check(A, t0 + secs(59)), Err(secs(1)));
        assert_eq!(limiter.check(A, t0 + secs(60)), Ok(()));
        limiter.failed(A, t0 + secs(60));
        assert_eq!(limiter.check(A, t0 + secs(60)), Err(secs(120)));
        limiter.failed(A, t0 + secs(180));
        assert_eq!(limiter.check(A, t0 + secs(180)), Err(secs(240)));
        // Other addresses are not affected.
        assert_eq!(limiter.check(B, t0), Ok(()));
    }

    #[test]
    fn failures_older_than_the_window_are_forgotten() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        for _ in 0..4 {
            limiter.failed(A, t0);
        }
        let later = t0 + secs(10 * 60);
        for _ in 0..4 {
            limiter.failed(A, later);
            assert_eq!(limiter.check(A, later), Ok(()));
        }
    }

    #[test]
    fn a_success_clears_the_count_and_the_lockout_is_capped() {
        let limiter = Limiter::new(Policy::ENROLL);
        let t0 = Instant::now();
        for _ in 0..4 {
            limiter.failed(A, t0);
        }
        limiter.succeeded(A);
        for _ in 0..4 {
            limiter.failed(A, t0);
        }
        assert_eq!(limiter.check(A, t0), Ok(()));
        for _ in 0..30 {
            limiter.failed(A, t0);
        }
        assert_eq!(limiter.check(A, t0), Err(secs(60 * 60)));
    }
}
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
pub mod hosts;
```

with:

```rust
pub mod hosts;
pub mod ratelimit;
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// 201 to `POST /api/hosts/pairing-codes` (kernel spec §4.1): a single-use
/// code for `hennery host join`, shown once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct PairingCodeResponse {
    /// `XXXX-XXXX`, Crockford base32.
    pub code: String,
    /// RFC 3339.
    pub expires_at: String,
}

/// `POST /api/hosts/enroll` (kernel spec §4.1): a host pairs itself with a
/// code and the public half of the key it generated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnrollRequest {
    pub code: String,
    /// Ed25519 public key, 64 hex characters.
    pub public_key: String,
    pub name: String,
    pub host_version: String,
    pub platform: String,
}

/// 201 to an enrollment: the id the host names itself by in `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct EnrollResponse {
    pub host_id: String,
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::AnswerRequest,
        rest::AnswerResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::AnswerRequest,
        rest::AnswerResponse,
        rest::PairingCodeResponse,
        rest::EnrollRequest,
        rest::EnrollResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::AnswerResponse,
    );
```

with:

```rust
        rest::AnswerResponse,
        rest::PairingCodeResponse,
        rest::EnrollRequest,
        rest::EnrollResponse,
    );
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
```

with:

```rust
pub(crate) fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
fn internal(err: anyhow::Error) -> Response {
```

with:

```rust
pub(crate) fn internal(err: anyhow::Error) -> Response {
```

Create `crates/hennery-sessions/src/hosts.rs`:

```rust
//! Host pairing endpoints (kernel spec §4.1, §8): minting pairing codes and
//! enrollment. They live beside the session API because the collector's
//! only HTTP router is here for now; the registry itself is the kernel's
//! (`hennery_kernel::hosts`).

use crate::AppState;
use crate::api::{error, internal};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router, middleware};
use hennery_kernel::hosts::{EnrollOutcome, Enrollment};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::{EnrollRequest, EnrollResponse, PairingCodeResponse};
use std::net::SocketAddr;
use std::time::Instant;

/// Routes that need an operator (the development bearer until operator
/// auth replaces it), and enrollment, which is authenticated by its code
/// alone and so sits outside that layer (kernel spec §3.3).
pub fn router(state: AppState) -> Router {
    let operator = Router::new()
        .route("/api/hosts/pairing-codes", post(mint_pairing_code))
        .layer(middleware::from_fn_with_state(
            state.token.clone(),
            hennery_kernel::auth::require_bearer,
        ));
    let code_authenticated = Router::new().route("/api/hosts/enroll", post(enroll));
    operator.merge(code_authenticated).with_state(state)
}

/// RFC 3339 for a kernel timestamp (seconds since the epoch).
pub(crate) fn rfc3339(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_default()
}

/// `POST /api/hosts/pairing-codes`: 201 with a fresh code.
async fn mint_pairing_code(State(state): State<AppState>) -> Response {
    match state.hosts.mint_pairing_code(unix_now()) {
        Ok(code) => (
            StatusCode::CREATED,
            Json(PairingCodeResponse {
                code: code.code,
                expires_at: rfc3339(code.expires_at),
            }),
        )
            .into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/hosts/enroll`: 201 `{host_id}`. A wrong code counts against
/// the client's address (kernel spec §4.1); once that address is locked
/// out, attempts are answered 429 without the code being looked at.
async fn enroll(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(req): Json<EnrollRequest>,
) -> Response {
    let now = Instant::now();
    if let Err(retry_after) = state.enroll_limiter.check(peer.ip(), now) {
        let mut response = error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many wrong pairing codes from this address; try again later",
        );
        let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        return response;
    }
    let enrollment = Enrollment {
        public_key: req.public_key,
        name: req.name,
        host_version: req.host_version,
        platform: req.platform,
    };
    match state.hosts.enroll(&req.code, &enrollment, unix_now()) {
        Ok(EnrollOutcome::Enrolled { host_id }) => {
            state.enroll_limiter.succeeded(peer.ip());
            tracing::info!(%host_id, name = %enrollment.name, "host paired");
            (StatusCode::CREATED, Json(EnrollResponse { host_id })).into_response()
        }
        Ok(EnrollOutcome::InvalidCode) => {
            state.enroll_limiter.failed(peer.ip(), now);
            error(
                StatusCode::UNAUTHORIZED,
                "invalid_code",
                "the pairing code is unknown, used or expired",
            )
        }
        Ok(EnrollOutcome::AlreadyPaired { .. }) => error(
            StatusCode::CONFLICT,
            "already_paired",
            "a host with this key is paired already",
        ),
        Ok(EnrollOutcome::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
pub mod api;
```

with:

```rust
pub mod api;
pub mod hosts;
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
use hennery_kernel::auth::DevToken;
```

with:

```rust
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::ratelimit::{Limiter, Policy};
use std::net::SocketAddr;
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    pub store: Arc<store::Store>,
```

with:

```rust
    pub store: Arc<store::Store>,
    /// The kernel's host registry (kernel spec §4).
    pub hosts: Arc<Hosts>,
    /// Wrong pairing codes per client address (kernel spec §4.1).
    pub enroll_limiter: Arc<Limiter>,
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    pub fn new(store: store::Store, token: DevToken) -> Self {
```

with:

```rust
    pub fn new(store: store::Store, hosts: Hosts, token: DevToken) -> Self {
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
            store: Arc::new(store),
```

with:

```rust
            store: Arc::new(store),
            hosts: Arc::new(hosts),
            enroll_limiter: Arc::new(Limiter::new(Policy::ENROLL)),
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
```

with:

```rust
    // The peer address is what enrollment rate-limits on (kernel spec §4.1).
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
/// Every session route plus the host WebSocket.
```

with:

```rust
/// Every session and host route plus the host WebSocket. Serve it with
/// `into_make_service_with_connect_info::<SocketAddr>()`: enrollment reads
/// the client's address.
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
    api::router(state.clone()).merge(ws::router(state))
```

with:

```rust
    api::router(state.clone())
        .merge(hosts::router(state.clone()))
        .merge(ws::router(state))
```

In `crates/hennery/src/main.rs`, replace:

```rust
use hennery_kernel::auth::DevToken;
```

with:

```rust
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery/src/main.rs`, replace:

```rust
use hennery_sessions::{AppState, store::Store};
```

with:

```rust
use hennery_sessions::{AppState, store::Store};
use std::net::SocketAddr;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let store = Store::open(&args.data_dir.join("hennery.db"))?;
    let mut state = AppState::new(store, DevToken::new(args.dev_token));
```

with:

```rust
    let db = args.data_dir.join("hennery.db");
    let store = Store::open(&db)?;
    let hosts = Hosts::open(&db)?;
    let mut state = AppState::new(store, hosts, DevToken::new(args.dev_token));
```

In `crates/hennery/src/main.rs`, replace:

```rust
    axum::serve(listener, app)
```

with:

```rust
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
use hennery_kernel::auth::DevToken;
```

with:

```rust
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/auth.rs`, replace:

```rust
            Store::open(&dir.path().join("hennery.db")).unwrap(),
```

with:

```rust
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
use hennery_kernel::auth::DevToken;
```

with:

```rust
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        let mut state = AppState::new(Store::open(db).unwrap(), DevToken::new(TOKEN));
```

with:

```rust
        let mut state = AppState::new(Store::open(db).unwrap(), Hosts::open(db).unwrap(), DevToken::new(TOKEN));
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use hennery_kernel::auth::DevToken;
```

with:

```rust
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            Store::open(&dir.path().join("hennery.db")).unwrap(),
```

with:

```rust
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use hennery_kernel::auth::DevToken;
```

with:

```rust
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, DevToken::new(TOKEN));
    let shutdown = state.shutdown.clone();
```

with:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, Hosts::open_in_memory().unwrap(), DevToken::new(TOKEN));
    let shutdown = state.shutdown.clone();
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    let state = AppState::new(store, DevToken::new(TOKEN));
```

with:

```rust
    let state = AppState::new(store, Hosts::open_in_memory().unwrap(), DevToken::new(TOKEN));
```

- [ ] **Step 4: Regenerate and run the new tests**

Run: `cargo run -p hennery-proto --bin gen`
Expected: `wrote schema/hennery-protocol.schema.json`, `wrote web/src/generated/protocol.ts`.

Run: `cargo test -p hennery-testkit --test pairing --locked && cargo test -p hennery-kernel --lib --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probe the layering**

Move `.route("/api/hosts/enroll", post(enroll))` into the `operator` router, above its `.layer(...)`, and rerun `cargo test -p hennery-testkit --test pairing --locked`. Expected: every test fails; `minting_needs_the_operator_and_enrollment_does_not` shows a 401 whose body is not JSON. Restore the code.

- [ ] **Step 6: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 332 tests pass.

- [ ] **Step 7: Commit and push**

```bash
git add crates schema web/src/generated
git commit -m "feat(sessions): pairing codes and code-authenticated host enrollment"
git push
```

### Task 3: The host's key and `hennery host join`

**Files:**
- Modify: `Cargo.toml` (`toml`), `crates/hennery-host/Cargo.toml`, `Cargo.lock`
- Create: `crates/hennery-host/src/identity.rs`, `crates/hennery-host/src/pairing.rs`
- Modify: `crates/hennery-host/src/lib.rs`, `crates/hennery/src/main.rs`
- Test: `crates/hennery-testkit/tests/join.rs`, `crates/hennery/tests/cli.rs`; unit tests in `identity.rs` and `pairing.rs`

**Interfaces:**
- Consumes: Task 2's `POST /api/hosts/enroll` and `EnrollRequest` / `EnrollResponse` / `ApiError`.
- Produces (`hennery_host::identity`):
  - `KEY_FILE = "host.key"`, `CONFIG_FILE = "host.toml"`;
  - `#[derive(Clone)] struct HostKey`, with `generate()`, `from_seed([u8; 32])`, `public_key_hex() -> String`, `sign_hello(nonce: &[u8], host_id, protocol_version) -> String`, `load(&Path) -> Result<Self>` and `save(&Path) -> Result<()>`. Its `Debug` shows only the public key;
  - `struct Paired { collector_url: String, host_id: String, key: HostKey }`, with `load(data_dir) -> Result<Option<Paired>>` and `save(data_dir) -> Result<()>`.
- Produces (`hennery_host::pairing`):
  - `enum Joined { Paired { host_id }, AlreadyPaired { host_id } }`;
  - `parse_public_url(&str) -> Result<reqwest::Url>` and `collector_ws_url(&str) -> Result<String>`;
  - `platform() -> String` and `default_name() -> String`;
  - `async join(public_url, code, data_dir: &Path, name) -> Result<Joined>`.
- Produces (CLI): `hennery host join <URL> <CODE> [--name NAME] --data-dir DIR` (or `HENNERY_HOST_DATA_DIR`).
- Interim, until Task 7: `join` treats a directory holding a pairing as paired without asking the collector; Task 7 asks it.

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-testkit/tests/join.rs`:

```rust
//! `hennery host join` against a real collector (kernel spec §4.1): the host
//! generates its key, enrolls it with a code and stores the pairing; a host
//! that is paired already is left alone.

use hennery_host::identity::{KEY_FILE, Paired};
use hennery_host::pairing::{Joined, join};
use hennery_kernel::auth::DevToken;
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HelloCheck, Hosts};
use hennery_proto::PROTOCOL_VERSION;
use hennery_proto::rest::PairingCodeResponse;
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;

const TOKEN: &str = "dev-token";

struct Collector {
    addr: SocketAddr,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&db).unwrap(),
            Hosts::open(&db).unwrap(),
            DevToken::new(TOKEN),
        );
        tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self { addr, state, _dir: dir }
    }

    fn public_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn mint(&self) -> String {
        let resp = reqwest::Client::new()
            .post(format!("{}/api/hosts/pairing-codes", self.public_url()))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 201);
        resp.json::<PairingCodeResponse>().await.unwrap().code
    }
}

#[tokio::test]
async fn joining_pairs_the_host_with_a_key_the_collector_accepts() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let code = collector.mint().await;
    let joined = join(&collector.public_url(), &code, dir.path(), "laptop")
        .await
        .unwrap();
    let Joined::Paired { host_id } = joined else {
        panic!("expected a new pairing, got {joined:?}");
    };
    let paired = Paired::load(dir.path()).unwrap().expect("pairing stored");
    assert_eq!(paired.host_id, host_id);
    assert_eq!(paired.collector_url, format!("ws://{}/api/hosts/ws", collector.addr));
    assert!(!dir.path().join(format!("{KEY_FILE}.pending")).exists());
    let record = collector.state.hosts.host(&host_id).unwrap().unwrap();
    assert_eq!(record.name, "laptop");
    let nonce = [9u8; 32];
    let proof = paired.key.sign_hello(&nonce, &host_id, PROTOCOL_VERSION);
    assert_eq!(
        collector
            .state
            .hosts
            .check_hello(&host_id, &nonce, PROTOCOL_VERSION, &proof)
            .unwrap(),
        HelloCheck::Accepted
    );
}

#[tokio::test]
async fn joining_again_leaves_the_pairing_as_it_is_and_spends_no_code() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let first = collector.mint().await;
    let Joined::Paired { host_id } = join(&collector.public_url(), &first, dir.path(), "laptop")
        .await
        .unwrap()
    else {
        panic!("expected a new pairing");
    };
    let key_before = std::fs::read(dir.path().join(KEY_FILE)).unwrap();

    let second = collector.mint().await;
    assert_eq!(
        join(&collector.public_url(), &second, dir.path(), "laptop")
            .await
            .unwrap(),
        Joined::AlreadyPaired {
            host_id: host_id.clone()
        }
    );
    assert_eq!(std::fs::read(dir.path().join(KEY_FILE)).unwrap(), key_before);
    assert_eq!(collector.state.hosts.list().unwrap().len(), 1);
    // The second code was never sent: it still pairs another host.
    let other = Enrollment {
        public_key: "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a".into(),
        name: "desk".into(),
        host_version: "0".into(),
        platform: "linux-x86_64".into(),
    };
    assert!(matches!(
        collector
            .state
            .hosts
            .enroll(&second, &other, hennery_kernel::secret::unix_now())
            .unwrap(),
        EnrollOutcome::Enrolled { .. }
    ));
}

#[tokio::test]
async fn a_wrong_code_fails_and_leaves_nothing_behind() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let err = join(&collector.public_url(), "0000-0000", dir.path(), "laptop")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown, used or expired"), "{err}");
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(collector.state.hosts.list().unwrap().is_empty());
}
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    for cmd in ["collector", "host", "up"] {
```

with:

```rust
    for cmd in ["collector", "host", "up"] {
        assert!(text.contains(cmd), "missing {cmd} in help:\n{text}");
    }
}

#[test]
fn host_help_lists_join_and_run() {
    let out = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["host", "--help"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in ["join", "run"] {
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --test join`
Expected: FAIL to compile. `error[E0432]: unresolved import hennery_host::identity` and `error[E0432]: unresolved import hennery_host::pairing`.

- [ ] **Step 3: The key, the stored pairing, `join` and the subcommand**

In `Cargo.toml`, replace:

```toml
tokio-util = { version = "0.7", features = ["compat"] }
```

with:

```toml
tokio-util = { version = "0.7", features = ["compat"] }
toml = "=1.1.6"
```

Replace the whole of `crates/hennery-host/Cargo.toml` with:

```toml
[package]
name = "hennery-host"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish.workspace = true

[features]
# Seams that let tests hold the session actor still (`session::test_hooks`).
# Enabled only by hennery-testkit's dev-dependency, never in a real build.
test-hooks = []

[dependencies]
agent-client-protocol.workspace = true
anyhow.workspace = true
ed25519-dalek.workspace = true
futures.workspace = true
getrandom.workspace = true
hennery-proto.workspace = true
hex.workspace = true
libc = "0.2"
reqwest.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tokio-tungstenite.workspace = true
tokio-util.workspace = true
toml.workspace = true
tracing.workspace = true
uuid.workspace = true

[dev-dependencies]
tempfile = "3"
```

Replace the whole of `crates/hennery-host/src/lib.rs` with:

```rust
//! The hennery host: runs ACP adapters for one machine and relays their
//! sessions to the collector (ACP core spec §2).

pub mod adapter;
pub mod connection;
pub mod identity;
pub mod outbox;
pub mod pairing;
pub mod session;
pub mod uplink;

pub use adapter::AgentCommand;
pub use connection::{HostConfig, run, run_until};
```

Create `crates/hennery-host/src/identity.rs`:

```rust
//! The host's identity (kernel spec §4.1, ACP core §3.5): the Ed25519 key it
//! generated when it paired, and the collector and host id it paired with.
//! Both live in the host's data directory (distribution spec §8):
//! `host.key` (the key's 32-byte seed in hex, mode 0600) and `host.toml`.

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const KEY_FILE: &str = "host.key";
pub const CONFIG_FILE: &str = "host.toml";

/// The host's private key.
#[derive(Clone)]
pub struct HostKey(SigningKey);

impl std::fmt::Debug for HostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HostKey({})", self.public_key_hex())
    }
}

impl HostKey {
    /// A new key from the operating system's CSPRNG.
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("the OS random number generator is available");
        Self::from_seed(seed)
    }

    /// The key a 32-byte seed determines (tests pin keys this way).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }

    /// The public half, as enrollment sends it: 64 hex characters.
    pub fn public_key_hex(&self) -> String {
        hex::encode(self.0.verifying_key().as_bytes())
    }

    /// `hello.proof`: the signature over the collector's nonce, the host id
    /// and the protocol version (`hennery_proto::hello_proof_message`), hex.
    pub fn sign_hello(&self, nonce: &[u8], host_id: &str, protocol_version: &str) -> String {
        let message = hennery_proto::hello_proof_message(nonce, host_id, protocol_version);
        hex::encode(self.0.sign(&message).to_bytes())
    }

    /// Read a key file written by `save`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let seed: [u8; 32] = hex::decode(text.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .with_context(|| format!("{} does not hold a host key", path.display()))?;
        Ok(Self::from_seed(seed))
    }

    /// Write the key to `path` with mode 0600, atomically (a temporary file
    /// in the same directory, then a rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(path, format!("{}\n", hex::encode(self.0.to_bytes())).as_bytes())
    }
}

/// `host.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HostToml {
    /// The collector's host WebSocket, e.g. `wss://c.example/api/hosts/ws`.
    collector: String,
    host_id: String,
}

/// A paired host: what `hennery host run` needs to connect.
#[derive(Debug, Clone)]
pub struct Paired {
    pub collector_url: String,
    pub host_id: String,
    pub key: HostKey,
}

impl Paired {
    /// The pairing stored in `data_dir`: `None` if the host was never
    /// paired there, an error if only half of it is there.
    pub fn load(data_dir: &Path) -> Result<Option<Self>> {
        let key_path = data_dir.join(KEY_FILE);
        let config_path = data_dir.join(CONFIG_FILE);
        match (key_path.exists(), config_path.exists()) {
            (false, false) => return Ok(None),
            (true, true) => {}
            _ => bail!(
                "{} holds only half a pairing ({KEY_FILE} and {CONFIG_FILE} go together); pair again with `hennery host join`",
                data_dir.display()
            ),
        }
        let text = std::fs::read_to_string(&config_path).with_context(|| format!("read {}", config_path.display()))?;
        let config: HostToml = toml::from_str(&text).with_context(|| format!("parse {}", config_path.display()))?;
        Ok(Some(Self {
            collector_url: config.collector,
            host_id: config.host_id,
            key: HostKey::load(&key_path)?,
        }))
    }

    /// Store the pairing in `data_dir`: the key first, then `host.toml`, so
    /// a `host.toml` is never there without its key.
    pub fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir)?;
        self.key.save(&data_dir.join(KEY_FILE))?;
        let config = toml::to_string(&HostToml {
            collector: self.collector_url.clone(),
            host_id: self.host_id.clone(),
        })?;
        write_private(&data_dir.join(CONFIG_FILE), config.as_bytes())
    }
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp: PathBuf = {
        let mut name = path.file_name().context("a file path")?.to_os_string();
        name.push(".tmp");
        path.with_file_name(name)
    };
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {} to {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Checked by the collector too (`hennery-kernel`'s `hosts` tests).
    const VECTOR_SIGNATURE: &str = "bd2b7388413c333e9ed69c330b4a8be8ffb6228609979b30607236fcdefab259\
cdf6b48fb39bfaa9b5a3cd01538280ec9e6d50c8831e9aae4d791f68112a6c04";

    #[test]
    fn the_host_signs_the_fixed_proof_vector() {
        let key = HostKey::from_seed([1; 32]);
        assert_eq!(key.sign_hello(&[2; 32], "host-1", "1.0"), VECTOR_SIGNATURE);
    }

    #[test]
    fn a_saved_pairing_loads_back_and_its_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Paired::load(dir.path()).unwrap().is_none());
        let paired = Paired {
            collector_url: "ws://127.0.0.1:7117/api/hosts/ws".into(),
            host_id: "host-1".into(),
            key: HostKey::generate(),
        };
        paired.save(dir.path()).unwrap();
        let loaded = Paired::load(dir.path()).unwrap().unwrap();
        assert_eq!(
            (loaded.collector_url.as_str(), loaded.host_id.as_str()),
            ("ws://127.0.0.1:7117/api/hosts/ws", "host-1")
        );
        assert_eq!(loaded.key.public_key_hex(), paired.key.public_key_hex());
        for file in [KEY_FILE, CONFIG_FILE] {
            let mode = std::fs::metadata(dir.path().join(file)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    #[test]
    fn half_a_pairing_is_an_error_and_the_key_never_shows_in_debug() {
        let dir = tempfile::tempdir().unwrap();
        let key = HostKey::from_seed([1; 32]);
        key.save(&dir.path().join(KEY_FILE)).unwrap();
        let err = Paired::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("half a pairing"), "{err}");
        let seed_hex = hex::encode([1u8; 32]);
        assert!(!format!("{key:?}").contains(&seed_hex));
    }
}
```

Create `crates/hennery-host/src/pairing.rs`:

```rust
//! `hennery host join <url> <code>` (kernel spec §4.1): generate the host's
//! key, enroll it with a pairing code, and store the pairing.

use crate::identity::{HostKey, KEY_FILE, Paired};
use anyhow::{Context, Result, bail};
use hennery_proto::rest::{ApiError, EnrollRequest, EnrollResponse};
use reqwest::Url;
use std::path::Path;
use std::time::Duration;

/// How long one enrollment request may take.
const ENROLL_TIMEOUT: Duration = Duration::from_secs(30);

/// What `join` did.
#[derive(Debug, Clone, PartialEq)]
pub enum Joined {
    /// Enrolled now, under this id.
    Paired { host_id: String },
    /// The data directory holds a pairing already; nothing was sent.
    AlreadyPaired { host_id: String },
}

/// The collector's base URL, checked: `https://`, or `http://` to a
/// loopback address only (umbrella spec §7.5), with no path.
pub fn parse_public_url(public_url: &str) -> Result<Url> {
    let url = Url::parse(public_url.trim()).with_context(|| format!("{public_url} is not a URL"))?;
    let Some(host) = url.host_str() else {
        bail!("{public_url} has no host");
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => bail!("{public_url}: plain http is only allowed to a loopback address; use https"),
        other => bail!("{public_url}: unsupported scheme {other}"),
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        bail!("{public_url}: give the collector's base URL, without a path");
    }
    Ok(url)
}

/// The host WebSocket of the collector at `public_url`.
pub fn collector_ws_url(public_url: &str) -> Result<String> {
    let mut url = parse_public_url(public_url)?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).expect("ws and wss are valid schemes here");
    url.set_path("/api/hosts/ws");
    Ok(url.to_string())
}

/// `<os>-<arch>`, as enrollment reports it.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// This machine's host name, the default name of a new host.
pub fn default_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname(2) writes at most `buf.len()` bytes into `buf`.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    match std::str::from_utf8(&buf[..len]) {
        Ok(name) if rc == 0 && !name.is_empty() => name.chars().take(64).collect(),
        _ => "host".into(),
    }
}

/// Pair the host whose data directory is `data_dir` with the collector at
/// `public_url`. Idempotent: a directory that is paired already is left as
/// it is, and the code is not spent.
pub async fn join(public_url: &str, code: &str, data_dir: &Path, name: &str) -> Result<Joined> {
    if let Some(paired) = Paired::load(data_dir)? {
        return Ok(Joined::AlreadyPaired {
            host_id: paired.host_id,
        });
    }
    let base = parse_public_url(public_url)?;
    let collector_url = collector_ws_url(public_url)?;
    std::fs::create_dir_all(data_dir)?;
    let key = HostKey::generate();
    // Written before enrolling, so a directory that cannot hold the key
    // never costs a code; renamed into place only once enrolled.
    let pending = data_dir.join(format!("{KEY_FILE}.pending"));
    key.save(&pending)?;
    let enrolled = enroll(&base, code, &key, name).await;
    let host_id = match enrolled {
        Ok(host_id) => host_id,
        Err(err) => {
            let _ = std::fs::remove_file(&pending);
            return Err(err);
        }
    };
    let paired = Paired {
        collector_url,
        host_id: host_id.clone(),
        key,
    };
    paired.save(data_dir)?;
    let _ = std::fs::remove_file(&pending);
    Ok(Joined::Paired { host_id })
}

async fn enroll(base: &Url, code: &str, key: &HostKey, name: &str) -> Result<String> {
    let url = base.join("/api/hosts/enroll").expect("an absolute path joins");
    let response = reqwest::Client::builder()
        .timeout(ENROLL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .post(url.clone())
        .json(&EnrollRequest {
            code: code.into(),
            public_key: key.public_key_hex(),
            name: name.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            platform: platform(),
        })
        .send()
        .await
        .with_context(|| format!("reach {url}"))?;
    let status = response.status();
    if status.as_u16() == 201 {
        return Ok(response.json::<EnrollResponse>().await?.host_id);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let message = match response.json::<ApiError>().await {
        Ok(e) => e.message,
        Err(_) => format!("the collector answered {status}"),
    };
    match (status.as_u16(), retry_after) {
        (429, Some(secs)) => bail!("{message} (retry in {secs} s)"),
        _ => bail!("pairing failed: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_websocket_is_derived_from_the_public_url() {
        assert_eq!(
            collector_ws_url("https://c.example").unwrap(),
            "wss://c.example/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("https://c.example:8443/").unwrap(),
            "wss://c.example:8443/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("http://127.0.0.1:7117").unwrap(),
            "ws://127.0.0.1:7117/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("http://localhost:7117").unwrap(),
            "ws://localhost:7117/api/hosts/ws"
        );
        assert_eq!(
            collector_ws_url("http://[::1]:7117").unwrap(),
            "ws://[::1]:7117/api/hosts/ws"
        );
    }

    #[test]
    fn plain_http_off_loopback_paths_and_other_schemes_are_refused() {
        for (url, why) in [
            ("http://c.example", "only allowed to a loopback"),
            ("http://192.168.1.2:7117", "only allowed to a loopback"),
            ("ftp://c.example", "unsupported scheme"),
            ("https://c.example/hennery", "without a path"),
            ("c.example", "not a URL"),
        ] {
            let err = collector_ws_url(url).unwrap_err().to_string();
            assert!(err.contains(why), "{url}: {err}");
        }
    }
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
use clap::{Args, Parser, Subcommand};
```

with:

```rust
use clap::{Args, Parser, Subcommand};
use hennery_host::pairing::Joined;
```

In `crates/hennery/src/main.rs`, replace:

```rust
enum HostCommand {
```

with:

```rust
enum HostCommand {
    /// Pair this machine with a collector (kernel spec §4.1).
    Join(JoinArgs),
```

In `crates/hennery/src/main.rs`, replace:

```rust
    Run(HostArgs),
```

with:

```rust
    Run(HostArgs),
}

#[derive(Args)]
struct JoinArgs {
    /// The collector's public URL, e.g. https://hennery.example.
    url: String,
    /// The pairing code shown by the collector (`XXXX-XXXX`).
    code: String,
    /// How the collector lists this host; defaults to the host name.
    #[arg(long)]
    name: Option<String>,
    #[arg(long, env = "HENNERY_HOST_DATA_DIR")]
    data_dir: PathBuf,
```

In `crates/hennery/src/main.rs`, replace:

```rust
        Command::Host {
```

with:

```rust
        Command::Host {
            command: HostCommand::Join(args),
        } => join_host(args).await,
        Command::Host {
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .await?;
```

with:

```rust
        .await?;
    Ok(())
}

async fn join_host(args: JoinArgs) -> Result<()> {
    let name = args.name.unwrap_or_else(hennery_host::pairing::default_name);
    match hennery_host::pairing::join(&args.url, &args.code, &args.data_dir, &name).await? {
        Joined::Paired { host_id } => println!("paired as {host_id}"),
        Joined::AlreadyPaired { host_id } => println!("already paired as {host_id}; nothing to do"),
    }
```

- [ ] **Step 4: Update the lock file and run the new tests**

Run: `cargo build --workspace` (records `toml` and the host's new dependencies), then `cargo test -p hennery-testkit --test join --locked && cargo test -p hennery-host --lib --locked && cargo test -p hennery --test cli host_help --locked`.
Expected: all pass, among them `the_host_signs_the_fixed_proof_vector` (the same vector as Task 1's verifier).

- [ ] **Step 5: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 341 tests pass.

- [ ] **Step 6: Commit and push**

```bash
git add Cargo.toml Cargo.lock crates
git commit -m "feat(host): host key, stored pairing and hennery host join"
git push
```

### Task 4: `host run` from the stored pairing, and `up` pairing its own host

**Files:**
- Create: `crates/hennery/src/inherit.rs`
- Modify: `crates/hennery/src/main.rs`, `crates/hennery/Cargo.toml` (`serde_json` for the tests), `Cargo.lock`
- Test: `crates/hennery/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1's `Hosts::mint_pairing_code`; Task 3's `Paired::load`, `pairing::{join, default_name}`.
- Produces (binary, private module `inherit`):
  - `CHILD_FD: RawFd = 3`;
  - `pass_to_child(&mut tokio::process::Command, &impl AsRawFd)`;
  - `write_code(RawFd, &str) -> Result<()>`, `async read_code(RawFd) -> Result<String>` and `close(RawFd)`.
- Produces (CLI):
  - `hennery host run --data-dir DIR [--agent …] [--idle-timeout-secs N]`, which reads `host.toml` and `host.key` and no longer takes `--collector` or `--host-id`;
  - hidden `--join-url URL --join-code-fd FD` on `host run`, and a hidden `--pairing-code-fd FD` on `collector`, for `up` only.
- Interim, until Task 5: `host run` still takes `--dev-token` for its `hello`, and `up` still passes it to the host child.

- [ ] **Step 1: Write the failing test**

In `crates/hennery/tests/cli.rs`, replace:

```rust
use std::net::TcpStream;
```

with:

```rust
use std::io::{Read, Write};
use std::net::TcpStream;
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
            "run",
            "--collector",
            "ws://x",
```

with:

```rust
            "run",
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        std::thread::sleep(Duration::from_millis(50));
    }
}
```

with:

```rust
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `GET path` on the collector with the development bearer: the JSON body
/// of a 200, else `None`.
fn get_json(listen: &str, path: &str, token: &str) -> Option<serde_json::Value> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {listen}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    if !head.starts_with("HTTP/1.1 200") {
        return None;
    }
    serde_json::from_str(body).ok()
}

fn free_listen() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    format!("127.0.0.1:{port}")
}

/// Removes a scratch directory on drop, however the test ends.
struct RemoveDir(std::path::PathBuf);

impl Drop for RemoveDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Start `up`, wait until its host is connected, and return the connected
/// host ids. The returned guard stops the whole tree (and leaves `dir`).
fn up_until_connected(listen: &str, dir: &std::path::Path) -> (KillTree, Vec<String>) {
    let up = Command::new(env!("CARGO_BIN_EXE_hennery"))
        .args(["up", "--listen", listen])
        .arg("--data-dir")
        .arg(dir)
        .args(["--dev-token", "t"])
        .spawn()
        .unwrap();
    let guard = KillTree {
        up,
        dir: std::path::PathBuf::new(),
        children: Vec::new(),
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(serde_json::Value::Array(hosts)) = get_json(listen, "/api/hosts", "t")
            && !hosts.is_empty()
        {
            let ids = hosts.iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
            return (guard, ids);
        }
        assert!(Instant::now() < deadline, "the all-in-one host never connected");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `hennery up` pairs its own host on first start, through the pipe the
/// supervisor hands both children (kernel spec §4.2), and a restart reuses
/// that pairing instead of minting another.
#[test]
fn up_pairs_its_own_host_once() {
    let listen = free_listen();
    let dir = std::env::temp_dir().join(format!(
        "hennery-cli-pair-{}-{}",
        std::process::id(),
        listen.replace(':', "-")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = RemoveDir(dir.clone());
    let host_dir = dir.join("host");

    let (mut first, ids) = up_until_connected(&listen, &dir);
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert!(ids[0].starts_with("host-"), "{ids:?}");
    let key = std::fs::read(host_dir.join("host.key")).unwrap();
    // SIGTERM, not a kill: `up` stops the host, then the collector.
    unsafe { libc::kill(first.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut first.up, Duration::from_secs(15)).is_some());

    let (_second, again) = up_until_connected(&listen, &dir);
    assert_eq!(again, ids, "the restart paired a second host");
    assert_eq!(std::fs::read(host_dir.join("host.key")).unwrap(), key);
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p hennery --test cli`
Expected: FAIL to compile. `error[E0433]: cannot find module or crate serde_json in this scope`: the binary's test dependency comes in Step 3.

- [ ] **Step 3: The pipe, and `host run` from the pairing**

Append to `crates/hennery/Cargo.toml`:

```toml
[dev-dependencies]
serde_json.workspace = true
```

Create `crates/hennery/src/inherit.rs`:

```rust
//! Handing the all-in-one host its pairing code (kernel spec §4.2): one pipe
//! from the collector child to the host child, each end inherited as a
//! file descriptor, so the code is never on a command line or in the
//! environment.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};

/// The descriptor number each child finds its end of the pipe at.
pub const CHILD_FD: RawFd = 3;

/// Make `fd` (one of this process's descriptors) the child's `CHILD_FD`,
/// open across `exec`. The pipe's descriptors are close-on-exec, so the
/// child inherits nothing else of the pipe.
pub fn pass_to_child(cmd: &mut tokio::process::Command, fd: &impl AsRawFd) {
    let fd = fd.as_raw_fd();
    // SAFETY: the closure runs in the forked child before `exec` and calls
    // only async-signal-safe functions (`dup2`, `fcntl`).
    unsafe {
        cmd.pre_exec(move || {
            if fd == CHILD_FD {
                // `dup2` onto itself would keep close-on-exec set.
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            } else if libc::dup2(fd, CHILD_FD) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// The collector's side: write the code and close the descriptor.
pub fn write_code(fd: RawFd, code: &str) -> Result<()> {
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    writeln!(file, "{code}").context("write the pairing code to the supervisor's pipe")?;
    Ok(())
}

/// The host's side: read the code the collector wrote. Blocks until the
/// collector has written it, or has exited without doing so.
pub async fn read_code(fd: RawFd) -> Result<String> {
    // SAFETY: `fd` was inherited for exactly this and nothing else owns it.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let text = tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        file.read_to_string(&mut text).map(|_| text)
    })
    .await?
    .context("read the pairing code from the supervisor's pipe")?;
    let code = text.trim().to_string();
    if code.is_empty() {
        bail!("the collector exited without handing over a pairing code");
    }
    Ok(code)
}

/// Close an inherited descriptor that is not needed after all.
pub fn close(fd: RawFd) {
    // SAFETY: as above; dropping the `File` closes it.
    drop(unsafe { std::fs::File::from_raw_fd(fd) });
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
use anyhow::{Context, Result};
```

with:

```rust
mod inherit;

use anyhow::{Context, Result, bail};
```

In `crates/hennery/src/main.rs`, replace:

```rust
use clap::{Args, Parser, Subcommand};
```

with:

```rust
use clap::{Args, Parser, Subcommand};
use hennery_host::identity::Paired;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    host_offline_secs: u64,
```

with:

```rust
    host_offline_secs: u64,
    /// `hennery up` only: once listening, write one pairing code to this
    /// inherited descriptor (kernel spec §4.2).
    #[arg(long, hide = true)]
    pairing_code_fd: Option<i32>,
```

In `crates/hennery/src/main.rs`, replace:

```rust
    /// e.g. ws://127.0.0.1:7117/api/hosts/ws
    #[arg(long)]
    collector: String,
    #[arg(long, default_value = "local")]
    host_id: String,
```

with:

```rust
    /// Holds the pairing `hennery host join` stored (`host.key`, `host.toml`).
```

In `crates/hennery/src/main.rs`, replace:

```rust
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
}

#[derive(Args)]
```

with:

```rust
    /// Park sessions idle for this many seconds; 0 turns the reaper off.
    #[arg(long, default_value_t = IDLE_TIMEOUT.as_secs())]
    idle_timeout_secs: u64,
    /// `hennery up` only: join this collector first if the host is not
    /// paired, with the code read from `--join-code-fd`.
    #[arg(long, hide = true, requires = "join_code_fd")]
    join_url: Option<String>,
    #[arg(long, hide = true, requires = "join_url")]
    join_code_fd: Option<i32>,
}

#[derive(Args)]
```

In `crates/hennery/src/main.rs`, replace:

```rust
    tracing::info!(address = %listener.local_addr()?, "collector listening");
```

with:

```rust
    tracing::info!(address = %listener.local_addr()?, "collector listening");
    if let Some(fd) = args.pairing_code_fd {
        // Only once migrated and listening: the host enrolls right away.
        let code = state.hosts.mint_pairing_code(hennery_kernel::secret::unix_now())?;
        inherit::write_code(fd, &code.code)?;
    }
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let mut cfg = HostConfig::new(args.collector, args.host_id, args.dev_token, args.data_dir);
```

with:

```rust
    let paired = match Paired::load(&args.data_dir)? {
        Some(paired) => {
            // Paired already (kernel spec §4.2): the code is not needed.
            if let Some(fd) = args.join_code_fd {
                inherit::close(fd);
            }
            paired
        }
        None => {
            let (Some(url), Some(fd)) = (&args.join_url, args.join_code_fd) else {
                bail!(
                    "{} holds no pairing; run `hennery host join <url> <code>` first",
                    args.data_dir.display()
                );
            };
            let code = inherit::read_code(fd).await?;
            let name = hennery_host::pairing::default_name();
            hennery_host::pairing::join(url, &code, &args.data_dir, &name).await?;
            Paired::load(&args.data_dir)?.context("the pairing just stored")?
        }
    };
    let mut cfg = HostConfig::new(paired.collector_url, paired.host_id, args.dev_token, args.data_dir);
```

In `crates/hennery/src/main.rs`, replace:

```rust
}

/// Two child processes of this binary, exchanging the same frames as a
```

with:

```rust
}

/// The collector's URL as its own host child reaches it: over loopback,
/// also when it listens on every interface.
fn loopback_url(listen: &str) -> String {
    match listen.parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_unspecified() && addr.is_ipv6() => format!("http://[::1]:{}", addr.port()),
        Ok(addr) if addr.ip().is_unspecified() => format!("http://127.0.0.1:{}", addr.port()),
        _ => format!("http://{listen}"),
    }
}

/// Two child processes of this binary, exchanging the same frames as a
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let exe = std::env::current_exe()?;
```

with:

```rust
    let exe = std::env::current_exe()?;
    let host_dir = args.data_dir.join("host");
    // The host pairs itself on first start only; a pairing that was revoked
    // is not replaced (kernel spec §4.2).
    let pairing = match Paired::load(&host_dir)? {
        Some(_) => None,
        None => Some(std::io::pipe()?),
    };
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let mut collector = tokio::process::Command::new(&exe)
```

with:

```rust
    let mut collector_cmd = tokio::process::Command::new(&exe);
    collector_cmd
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .process_group(0)
        .spawn()?;
```

with:

```rust
        .process_group(0);
    if let Some((_, writer)) = &pairing {
        inherit::pass_to_child(&mut collector_cmd, writer);
        collector_cmd
            .arg("--pairing-code-fd")
            .arg(inherit::CHILD_FD.to_string());
    }
    let mut collector = collector_cmd.spawn()?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .args([
            "host",
            "run",
            "--collector",
            &format!("ws://{}/api/hosts/ws", args.listen),
        ])
```

with:

```rust
        .args(["host", "run"])
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .arg(args.data_dir.join("host"))
```

with:

```rust
        .arg(&host_dir)
```

In `crates/hennery/src/main.rs`, replace:

```rust
    }
    let mut host = host_cmd.spawn()?;
```

with:

```rust
    }
    if let Some((reader, _)) = &pairing {
        inherit::pass_to_child(&mut host_cmd, reader);
        host_cmd
            .arg("--join-url")
            .arg(loopback_url(&args.listen))
            .arg("--join-code-fd")
            .arg(inherit::CHILD_FD.to_string());
    }
    let mut host = host_cmd.spawn()?;
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let mut host = host_cmd.spawn()?;
```

with:

```rust
    let mut host = host_cmd.spawn()?;
    // Both children hold their ends now; with the supervisor's copies
    // closed, the host sees end-of-file if the collector dies first.
    drop(pairing);
```

- [ ] **Step 4: Update the lock file and run the tests**

Run: `cargo build --workspace`, then `cargo test -p hennery --test cli --locked`.
Expected: 6 pass, `up_pairs_its_own_host_once` in about two seconds.

- [ ] **Step 5: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 342 tests pass.

- [ ] **Step 6: Commit and push**

```bash
git add Cargo.lock crates/hennery
git commit -m "feat(cli): host run from the stored pairing; up pairs its own host"
git push
```

### Task 5: The `hello` proof replaces the development token

**Files:**
- Modify: `crates/hennery-proto/src/frames.rs` (`hello.proof`), `rest.rs` (`HostItem`), `codegen.rs`
- Modify: `crates/hennery-host/src/connection.rs` (`HostConfig.key`, signing), `crates/hennery-sessions/src/ws.rs` (nonce, check, `record_hello`), `crates/hennery-sessions/src/hosts.rs` and `api.rs` (`GET /api/hosts` moves and returns `HostItem`s), `crates/hennery/src/main.rs` (no `--dev-token` for hosts)
- Modify: `crates/hennery-sessions/Cargo.toml`, `crates/hennery-testkit/Cargo.toml` (`hex`), `Cargo.lock`
- Modify (every harness signs its `hello` and pairs `host-1` with a fixed key): `crates/hennery-proto/tests/frames.rs`, `crates/hennery-testkit/tests/{e2e,reconcile,ws_ingest_error,host_connection}.rs`, `crates/hennery/tests/cli.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-testkit/tests/auth.rs` (rewritten), `crates/hennery-testkit/tests/host_connection.rs`

**Interfaces:**
- Consumes: Task 1's `Hosts::{check_hello, record_hello, register}`, `HELLO_NONCE_HEADER`; Task 3's `HostKey`.
- Produces: `HostFrame::Hello { protocol_version, host_version, host_id, proof: String, capabilities, attached_sessions }`, where `token` is gone.
- Produces: `HostConfig::new(collector_url, host_id, key: HostKey, data_dir: PathBuf)`, with the field `key: HostKey` in place of `token`.
- Produces: `HostItem { host_id, name, platform, host_version: String, capabilities: Capabilities, connected: bool, created_at: String, last_seen_at: Option<String>, revoked_at: Option<String> }`. `GET /api/hosts` (bearer) now answers `Vec<HostItem>`.
- Produces: `pub(crate) hosts::host_item(&AppState, HostRecord) -> HostItem`.
- Test harnesses pair `host-1` with `HostKey::from_seed([1; 32])` through `Hosts::register`. The fake collectors in `host_connection.rs` accept with the nonce header (`accept`).

- [ ] **Step 1: Write the failing tests**

Replace the whole of `crates/hennery-testkit/tests/auth.rs` with:

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
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::rest::HostItem;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
use hennery_sessions::AppState;
use hennery_sessions::store::Store;
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "dev-token";
const HOST: &str = "host-1";

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

struct Collector {
    addr: SocketAddr,
    state: AppState,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    _dir: tempfile::TempDir,
}

impl Collector {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(
            Store::open(&dir.path().join("hennery.db")).unwrap(),
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
            DevToken::new(TOKEN),
        );
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        state.hosts.register(HOST, &enrollment, 0).unwrap();
        let task = tokio::spawn(hennery_sessions::serve(listener, state.clone()));
        Self {
            addr,
            state,
            task,
            _dir: dir,
        }
    }

    async fn stop(self) {
        self.state.shutdown.cancel();
        self.task.await.unwrap().unwrap();
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn connected(&self) -> Vec<String> {
        self.state.hub.connected_hosts()
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Open a host socket; the nonce is the one in the upgrade response.
async fn connect(collector: &Collector) -> (Ws, Vec<u8>) {
    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
        .await
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    assert_eq!(nonce.len(), 32);
    (ws, nonce)
}

/// Send a `hello` for `host_id` with `proof` and return the answer.
async fn hello(ws: &mut Ws, host_id: &str, proof: String) -> CollectorFrame {
    ws.send(Message::text(
        serde_json::to_string(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "0".into(),
            host_id: host_id.into(),
            proof,
            capabilities: Default::default(),
            attached_sessions: vec![],
        })
        .unwrap(),
    ))
    .await
    .unwrap();
    match ws.next().await {
        Some(Ok(Message::Text(t))) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected a reply to hello, got {other:?}"),
    }
}

fn hello_error(frame: &CollectorFrame) -> &str {
    match frame {
        CollectorFrame::HelloError { code, .. } => code,
        other => panic!("expected hello_error, got {other:?}"),
    }
}

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
}

#[tokio::test]
async fn a_hello_signed_by_another_key_is_rejected_without_registering() {
    let collector = Collector::start().await;
    let (mut ws, nonce) = connect(&collector).await;
    let forged = HostKey::from_seed([2; 32]).sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    let reply = hello(&mut ws, HOST, forged).await;
    assert_eq!(hello_error(&reply), "bad_proof");
    // The socket is closed after the rejection, and the host never
    // registered.
    assert!(matches!(ws.next().await, None | Some(Err(_))));
    assert!(collector.connected().is_empty());
    collector.stop().await;
}

#[tokio::test]
async fn a_proof_is_good_on_its_own_connection_only() {
    let collector = Collector::start().await;
    let (_first, first_nonce) = connect(&collector).await;
    let (mut second, second_nonce) = connect(&collector).await;
    assert_ne!(first_nonce, second_nonce);
    // Replaying the first connection's proof on the second is refused.
    let replayed = host_key().sign_hello(&first_nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut second, HOST, replayed).await), "bad_proof");

    let (mut third, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    let reply = hello(&mut third, HOST, proof).await;
    assert!(matches!(reply, CollectorFrame::HelloAck { .. }), "{reply:?}");
    let record = collector.state.hosts.host(HOST).unwrap().unwrap();
    assert_eq!(record.host_version, "0");
    assert!(record.last_seen_at.is_some());
    collector.stop().await;
}

#[tokio::test]
async fn an_unknown_host_is_refused_like_a_bad_proof() {
    let collector = Collector::start().await;
    let (mut ws, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, "host-9", PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, "host-9", proof).await), "bad_proof");
    collector.stop().await;
}

#[tokio::test]
async fn a_revoked_host_is_told_so_but_only_with_a_valid_proof() {
    let collector = Collector::start().await;
    collector.state.hosts.revoke(HOST, 1).unwrap();

    let (mut ws, nonce) = connect(&collector).await;
    let forged = HostKey::from_seed([2; 32]).sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, HOST, forged).await), "bad_proof");

    let (mut ws, nonce) = connect(&collector).await;
    let proof = host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION);
    assert_eq!(hello_error(&hello(&mut ws, HOST, proof).await), "revoked");
    assert!(collector.connected().is_empty());
    collector.stop().await;
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-testkit --test auth`
Expected: FAIL to compile. `error[E0559]: variant HostFrame::Hello has no field named proof`, `error[E0432]: unresolved import hennery_proto::rest::HostItem` and `error[E0433]: cannot find module or crate hex in this scope` (the testkit's `hex` comes in Step 3).

- [ ] **Step 3: Sign and check `hello`, and list the registry**

In `crates/hennery-sessions/Cargo.toml`, replace:

```toml
hennery-proto.workspace = true
```

with:

```toml
hennery-proto.workspace = true
hex.workspace = true
```

In `crates/hennery-testkit/Cargo.toml`, replace:

```toml
hennery-sessions.workspace = true
```

with:

```toml
hennery-sessions.workspace = true
hex.workspace = true
```

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
        /// Walking skeleton only: a shared development token. Replaced by an
        /// Ed25519 proof of possession (ACP core §3.5).
        token: String,
```

with:

```rust
        /// Proof of possession of the host's key (ACP core §3.5): its
        /// Ed25519 signature, hex, over `hello_proof_message(nonce, host_id,
        /// protocol_version)`, where the nonce is the one the collector sent
        /// in the upgrade response's `hennery-hello-nonce` header.
        proof: String,
```

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// One entry of `GET /api/hosts` (kernel spec §8): a paired host, revoked
/// ones included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct HostItem {
    pub host_id: String,
    pub name: String,
    pub platform: String,
    pub host_version: String,
    /// From its latest accepted `hello`.
    pub capabilities: crate::frames::Capabilities,
    /// Connected and reconciled: requests reach it now.
    pub connected: bool,
    /// RFC 3339.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub revoked_at: Option<String>,
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::EnrollRequest,
        rest::EnrollResponse,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::EnrollRequest,
        rest::EnrollResponse,
        rest::HostItem,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::EnrollResponse,
    );
```

with:

```rust
        rest::EnrollResponse,
        rest::HostItem,
    );
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
            "token": "t", "attached_sessions": []
```

with:

```rust
            "proof": "p", "attached_sessions": []
```

In `crates/hennery-proto/tests/frames.rs`, replace:

```rust
        token: "t".into(),
```

with:

```rust
        proof: "p".into(),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
//! writing to the outbox, which is resent on the next connection.

use crate::outbox::Outbox;
```

with:

```rust
//! writing to the outbox, which is resent on the next connection.

use crate::identity::HostKey;
use crate::outbox::Outbox;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
use futures::{SinkExt, StreamExt};
use hennery_proto::PROTOCOL_VERSION;
```

with:

```rust
use futures::{SinkExt, StreamExt};
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionConfig};
```

with:

```rust
use hennery_proto::frames::{AttachedSession, Capabilities, Capability, CollectorFrame, HostFrame, SessionConfig};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    /// Walking skeleton: shared development token (ACP core §3.5 replaces it).
    pub token: String,
```

with:

```rust
    /// The key this host paired with; it signs every `hello` (ACP core §3.5).
    pub key: HostKey,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    pub fn new(
        collector_url: impl Into<String>,
        host_id: impl Into<String>,
        token: impl Into<String>,
        data_dir: PathBuf,
    ) -> Self {
```

with:

```rust
    pub fn new(collector_url: impl Into<String>, host_id: impl Into<String>, key: HostKey, data_dir: PathBuf) -> Self {
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            token: token.into(),
```

with:

```rust
            key,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let (ws, _) = tokio::time::timeout(
```

with:

```rust
    let (ws, response) = tokio::time::timeout(
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    .context("connect to collector")?;
```

with:

```rust
    .context("connect to collector")?;
    // The proof is over this connection's nonce, so it cannot be replayed on
    // another one (ACP core §3.5). Without one there is nothing to sign.
    let nonce = response
        .headers()
        .get(HELLO_NONCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| hex::decode(v).ok())
        .filter(|n| n.len() == 32)
        .context("the collector sent no hello nonce")?;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            token: cfg.token.clone(),
```

with:

```rust
            proof: cfg.key.sign_hello(&nonce, &cfg.host_id, PROTOCOL_VERSION),
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
```

with:

```rust
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderValue;
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
use futures::{SinkExt, StreamExt};
```

with:

```rust
use futures::{SinkExt, StreamExt};
use hennery_kernel::hosts::HelloCheck;
use hennery_kernel::secret::{random_bytes, unix_now};
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
use hennery_proto::{PROTOCOL_VERSION, protocol_major};
```

with:

```rust
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION, protocol_major};
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
```

with:

```rust
}

/// The upgrade response carries this connection's nonce: the host signs it
/// in `hello` (ACP core §3.5), so a proof is good for one connection only.
async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    ws.max_message_size(MAX_FRAME)
```

with:

```rust
    let nonce = random_bytes::<32>();
    let mut response = ws
        .max_message_size(MAX_FRAME)
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        .on_upgrade(move |socket| serve(socket, state))
```

with:

```rust
        .on_upgrade(move |socket| serve(socket, state, nonce));
    response.headers_mut().insert(
        HELLO_NONCE_HEADER,
        HeaderValue::from_str(&hex::encode(nonce)).expect("hex is a valid header value"),
    );
    response
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
async fn serve(socket: WebSocket, state: AppState) {
```

with:

```rust
async fn serve(socket: WebSocket, state: AppState, nonce: [u8; 32]) {
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        protocol_version,
```

with:

```rust
        protocol_version,
        host_version,
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        token,
```

with:

```rust
        proof,
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        attached_sessions,
        ..
```

with:

```rust
        attached_sessions,
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    if !state.token.matches(&token) {
        // The dev token stands in for ACP core §3.3's proof of identity.
        let _ = sink.send(text(&reject("bad_proof", "invalid host credential"))).await;
        return;
```

with:

```rust
    // The host id is never taken on its word (ACP core §3.5).
    match state.hosts.check_hello(&host_id, &nonce, &protocol_version, &proof) {
        Ok(HelloCheck::Accepted) => {}
        Ok(HelloCheck::Revoked) => {
            tracing::warn!(%host_id, "revoked host tried to connect");
            let _ = sink
                .send(text(&reject(
                    "revoked",
                    "this host was revoked; pair it again with `hennery host join`",
                )))
                .await;
            return;
        }
        Ok(HelloCheck::BadProof) => {
            tracing::warn!(%host_id, "hello with an unknown host id or an invalid proof");
            let _ = sink
                .send(text(&reject("bad_proof", "unknown host or invalid proof")))
                .await;
            return;
        }
        Err(err) => {
            tracing::error!(%host_id, error = %err, "checking a hello failed");
            return;
        }
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    let Some(registration) = state.hub.register(&host_id, tx.clone(), capabilities) else {
```

with:

```rust
    let Some(registration) = state.hub.register(&host_id, tx.clone(), capabilities.clone()) else {
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        crate::offline::after_disconnect(&state, host_id, conn_id);
        return;
    }
```

with:

```rust
        crate::offline::after_disconnect(&state, host_id, conn_id);
        return;
    }
    if let Err(err) = state
        .hosts
        .record_hello(&host_id, &host_version, &capabilities, unix_now())
    {
        tracing::warn!(%host_id, error = %err, "recording the host's hello failed");
    }
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    Router::new()
        .route("/api/hosts", get(list_hosts))
```

with:

```rust
    Router::new()
```

In `crates/hennery-sessions/src/api.rs`, replace:

```rust
    }
}

async fn list_hosts(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.hub.connected_hosts())
```

with:

```rust
    }
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use axum::routing::post;
```

with:

```rust
use axum::routing::{get, post};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment};
```

with:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use hennery_proto::rest::{EnrollRequest, EnrollResponse, PairingCodeResponse};
```

with:

```rust
use hennery_proto::rest::{EnrollRequest, EnrollResponse, HostItem, PairingCodeResponse};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
    let operator = Router::new()
```

with:

```rust
    let operator = Router::new()
        .route("/api/hosts", get(list_hosts))
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
        .unwrap_or_default()
```

with:

```rust
        .unwrap_or_default()
}

/// A registry entry as the API shows it, with whether it is connected.
pub(crate) fn host_item(state: &AppState, record: HostRecord) -> HostItem {
    HostItem {
        connected: state.hub.is_ready(&record.id),
        host_id: record.id,
        name: record.name,
        platform: record.platform,
        host_version: record.host_version,
        capabilities: record.capabilities,
        created_at: rfc3339(record.created_at),
        last_seen_at: record.last_seen_at.map(rfc3339),
        revoked_at: record.revoked_at.map(rfc3339),
    }
}

/// `GET /api/hosts`: every paired host, oldest first.
async fn list_hosts(State(state): State<AppState>) -> Response {
    match state.hosts.list() {
        Ok(records) => {
            let items: Vec<HostItem> = records.into_iter().map(|r| host_item(&state, r)).collect();
            Json(items).into_response()
        }
        Err(err) => internal(err),
    }
```

In `crates/hennery/src/main.rs`, replace:

```rust
//! The `hennery` binary (distribution spec §1). Walking skeleton: collector,
//! host and an all-in-one mode, with a shared development token.
```

with:

```rust
//! The `hennery` binary (distribution spec §1): collector, host (join and
//! run) and an all-in-one mode. Hosts authenticate with the key they paired
//! with; the REST API still takes a development bearer token until operator
//! auth lands.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    /// Development token for hosts and API clients (skeleton only).
```

with:

```rust
    /// Development bearer token for REST clients, until operator auth.
    /// Hosts authenticate with their paired key instead.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    data_dir: PathBuf,
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
    dev_token: String,
    /// Agent adapter, as `name=command args…`. Repeatable.
```

with:

```rust
    data_dir: PathBuf,
    /// Agent adapter, as `name=command args…`. Repeatable.
```

In `crates/hennery/src/main.rs`, replace:

```rust
    data_dir: PathBuf,
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
```

with:

```rust
    data_dir: PathBuf,
    /// The collector's development bearer token for REST clients.
    #[arg(long, env = "HENNERY_DEV_TOKEN", hide_env_values = true)]
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let mut cfg = HostConfig::new(paired.collector_url, paired.host_id, args.dev_token, args.data_dir);
```

with:

```rust
    let mut cfg = HostConfig::new(paired.collector_url, paired.host_id, paired.key, args.data_dir);
```

In `crates/hennery/src/main.rs`, replace:

```rust
        .arg(args.idle_timeout_secs.to_string())
        .env("HENNERY_DEV_TOKEN", &args.dev_token)
```

with:

```rust
        .arg(args.idle_timeout_secs.to_string())
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
        .args([
            "host",
            "run",
            "--data-dir",
            "/tmp/x",
            "--dev-token",
            "t",
            "--agent",
            "noequals",
        ])
```

with:

```rust
        .args(["host", "run", "--data-dir", "/tmp/x", "--agent", "noequals"])
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
            && !hosts.is_empty()
```

with:

```rust
            && hosts.iter().any(|h| h["connected"] == true)
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
            let ids = hosts.iter().filter_map(|h| h.as_str().map(str::to_string)).collect();
```

with:

```rust
            let ids = hosts
                .iter()
                .filter_map(|h| h["host_id"].as_str().map(str::to_string))
                .collect();
```

- [ ] **Step 4: Sign every test harness's `hello`**

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
//! fake ACP adapter as a real child process, talking over real sockets.

use hennery_host::{AgentCommand, HostConfig};
```

with:

```rust
//! fake ACP adapter as a real child process, talking over real sockets.

use hennery_host::identity::HostKey;
use hennery_host::{AgentCommand, HostConfig};
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_proto::rest::{EventDto, PromptResponse, StartSessionResponse};
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_proto::rest::{EventDto, HostItem, PromptResponse, StartSessionResponse};
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
const TOKEN: &str = "dev-token";
```

with:

```rust
const TOKEN: &str = "dev-token";

/// The key `host-1` is paired with in every collector here.
fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

/// Pair `host-1` (a no-op for a collector restarted over the same database).
fn pair_host(hosts: &Hosts) {
    let enrollment = Enrollment {
        public_key: host_key().public_key_hex(),
        name: "test".into(),
        host_version: "test".into(),
        platform: "test".into(),
    };
    hosts.register("host-1", &enrollment, 0).unwrap();
}
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        let mut state = AppState::new(Store::open(db).unwrap(), Hosts::open(db).unwrap(), DevToken::new(TOKEN));
```

with:

```rust
        let hosts = Hosts::open(db).unwrap();
        pair_host(&hosts);
        let mut state = AppState::new(Store::open(db).unwrap(), hosts, DevToken::new(TOKEN));
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        TOKEN,
```

with:

```rust
        host_key(),
```

In `crates/hennery-testkit/tests/e2e.rs`, replace:

```rust
        let hosts: Vec<String> = c.get(&url).send().await.ok()?.json().await.ok()?;
        hosts.contains(&"host-1".to_string()).then_some(())
```

with:

```rust
        let hosts: Vec<HostItem> = c.get(&url).send().await.ok()?.json().await.ok()?;
        hosts.iter().any(|h| h.host_id == "host-1" && h.connected).then_some(())
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use futures::{SinkExt, StreamExt};
```

with:

```rust
use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_proto::PROTOCOL_VERSION;
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
use hennery_proto::rest::EventDto;
```

with:

```rust
use hennery_proto::rest::EventDto;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
const HOST: &str = "host-1";
```

with:

```rust
const HOST: &str = "host-1";

/// The key `HOST` is paired with.
fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        let addr = listener.local_addr().unwrap();
```

with:

```rust
        let addr = listener.local_addr().unwrap();
        let hosts = Hosts::open(&dir.path().join("hennery.db")).unwrap();
        let enrollment = Enrollment {
            public_key: host_key().public_key_hex(),
            name: "test".into(),
            host_version: "test".into(),
            platform: "test".into(),
        };
        // A no-op when the directory is reused (a collector restart).
        hosts.register(HOST, &enrollment, 0).unwrap();
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            Hosts::open(&dir.path().join("hennery.db")).unwrap(),
```

with:

```rust
            hosts,
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
```

with:

```rust
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            .unwrap();
        let mut host = Self { ws, seq };
```

with:

```rust
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws, seq };
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
            token: TOKEN.into(),
```

with:

```rust
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
```

with:

```rust
    let (mut ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        .unwrap();
    let hello = json!({
```

with:

```rust
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    let hello = json!({
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        "host_id": HOST, "token": TOKEN, "capabilities": ["teleport", "park"], "attached_sessions": []
```

with:

```rust
        "host_id": HOST, "proof": host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
        "capabilities": ["teleport", "park"], "attached_sessions": []
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use futures::{SinkExt, StreamExt};
```

with:

```rust
use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use hennery_kernel::hosts::Hosts;
use hennery_proto::PROTOCOL_VERSION;
```

with:

```rust
use hennery_kernel::hosts::{Enrollment, Hosts};
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame, SessionBody};
```

with:

```rust
use hennery_proto::frames::{Capabilities, CollectorFrame, HostFrame, SessionBody};
use hennery_proto::rest::HostItem;
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
}

#[tokio::test]
```

with:

```rust
}

fn host_key() -> HostKey {
    HostKey::from_seed([1; 32])
}

/// A registry with `host-1` paired, in memory: the tests below lock the
/// session database, and the registry must not wait on that lock.
fn paired_hosts() -> Hosts {
    let hosts = Hosts::open_in_memory().unwrap();
    let enrollment = Enrollment {
        public_key: host_key().public_key_hex(),
        name: "test".into(),
        host_version: "test".into(),
        platform: "test".into(),
    };
    hosts.register("host-1", &enrollment, 0).unwrap();
    hosts
}

#[tokio::test]
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, Hosts::open_in_memory().unwrap(), DevToken::new(TOKEN));
    let shutdown = state.shutdown.clone();
```

with:

```rust
    store.create_session("s1", "host-1", "fake", "/tmp").unwrap();

    let state = AppState::new(store, paired_hosts(), DevToken::new(TOKEN));
    let shutdown = state.shutdown.clone();
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    .unwrap();

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
```

with:

```rust
    .unwrap();

    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
        .unwrap();
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
```

with:

```rust
    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
        .await
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::text(
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
            host_id: "host-1".into(),
            token: TOKEN.into(),
            capabilities: Default::default(),
```

with:

```rust
            host_id: "host-1".into(),
            proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
            capabilities: Default::default(),
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    let state = AppState::new(store, Hosts::open_in_memory().unwrap(), DevToken::new(TOKEN));
```

with:

```rust
    let state = AppState::new(store, paired_hosts(), DevToken::new(TOKEN));
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
```

with:

```rust
    let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/hosts/ws"))
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
        .unwrap();
    let (mut sink, mut stream) = ws.split();
```

with:

```rust
        .unwrap();
    let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
    let (mut sink, mut stream) = ws.split();
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
            token: TOKEN.into(),
```

with:

```rust
            proof: host_key().sign_hello(&nonce, "host-1", PROTOCOL_VERSION),
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
        let hosts: Vec<String> = client
```

with:

```rust
        let hosts: Vec<HostItem> = client
```

In `crates/hennery-testkit/tests/ws_ingest_error.rs`, replace:

```rust
        if hosts.iter().any(|h| h == "host-1") {
```

with:

```rust
        if hosts.iter().any(|h| h.host_id == "host-1" && h.connected) {
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
use futures::{SinkExt, StreamExt};
```

with:

```rust
use futures::{SinkExt, StreamExt};
use hennery_host::identity::HostKey;
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
use hennery_host::{HostConfig, run};
use hennery_proto::PROTOCOL_VERSION;
```

with:

```rust
use hennery_host::{HostConfig, run};
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
use hennery_proto::frames::{CollectorFrame, HostFrame};
```

with:

```rust
use hennery_proto::frames::{CollectorFrame, HostFrame};
use hennery_proto::{HELLO_NONCE_HEADER, PROTOCOL_VERSION};
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        committed: BTreeMap::new(),
    }
}
```

with:

```rust
        committed: BTreeMap::new(),
    }
}

/// Accept a host's WebSocket the way the collector does: the upgrade
/// response carries a hello nonce (ACP core §3.5). These fake collectors
/// check no proof, so any nonce does.
async fn accept(
    tcp: tokio::net::TcpStream,
) -> Result<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, tokio_tungstenite::tungstenite::Error> {
    tokio_tungstenite::accept_hdr_async(tcp, with_nonce).await
}

/// The signature tungstenite's handshake callback requires.
#[allow(clippy::result_large_err)]
fn with_nonce(
    _: &tokio_tungstenite::tungstenite::handshake::server::Request,
    mut response: tokio_tungstenite::tungstenite::handshake::server::Response,
) -> Result<
    tokio_tungstenite::tungstenite::handshake::server::Response,
    tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
> {
    response
        .headers_mut()
        .insert(HELLO_NONCE_HEADER, hex::encode([5u8; 32]).parse().unwrap());
    Ok(response)
}
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
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
            hellos.send(()).ok();
```

with:

```rust
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = accept(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(()).ok();
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        "host1",
        "token",
        unique_data_dir("read-deadline"),
```

with:

```rust
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir("read-deadline"),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
async fn ack_then_close_server(listener: TcpListener, hellos: mpsc::UnboundedSender<Instant>) {
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
```

with:

```rust
async fn ack_then_close_server(listener: TcpListener, hellos: mpsc::UnboundedSender<Instant>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = accept(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(Instant::now()).ok();
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
async fn ack_hello_only_then_close_server(listener: TcpListener, hellos: mpsc::UnboundedSender<Instant>) {
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
```

with:

```rust
async fn ack_hello_only_then_close_server(listener: TcpListener, hellos: mpsc::UnboundedSender<Instant>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let hellos = hellos.clone();
        tokio::spawn(async move {
            let Ok(ws) = accept(tcp).await else {
                return;
            };
            let (mut sink, mut stream) = ws.split();
            let hello = read_host_frame(&mut stream).await;
            assert!(matches!(hello, HostFrame::Hello { .. }), "{hello:?}");
            hellos.send(Instant::now()).ok();
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        "host1",
        "token",
        unique_data_dir("backoff-reset"),
```

with:

```rust
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir("backoff-reset"),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        "host1",
        "token",
        unique_data_dir("backoff-no-ack"),
```

with:

```rust
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir("backoff-no-ack"),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
    let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
```

with:

```rust
    let ws = accept(tcp).await.unwrap();
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        "host1",
        "token",
        unique_data_dir(name),
```

with:

```rust
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir(name),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
```

with:

```rust
            let Ok(ws) = accept(tcp).await else {
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        "host1",
        "token",
        unique_data_dir("healthy-reset"),
```

with:

```rust
        "host1",
        HostKey::from_seed([1; 32]),
        unique_data_dir("healthy-reset"),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
        "token",
```

with:

```rust
        HostKey::from_seed([1; 32]),
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
    let (_sink, mut stream) = tokio_tungstenite::accept_async(tcp).await.unwrap().split();
```

with:

```rust
    let (_sink, mut stream) = accept(tcp).await.unwrap().split();
```

In `crates/hennery-testkit/tests/host_connection.rs`, replace:

```rust
    send_frame(&mut sink, &answer("r4", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}
```

with:

```rust
    send_frame(&mut sink, &answer("r4", "no-such-session")).await;
    let refused = read_until(&mut stream, error_for("r4")).await;
    assert!(
        matches!(&refused, HostFrame::Error { code, .. } if code == "not_attached"),
        "{refused:?}"
    );
}

// Plan 3a: the hello proof.

#[tokio::test]
async fn a_host_signs_its_hello_over_the_connections_nonce() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run(host_with_fake(addr, "proof", slow_fake())));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    let (_sink, mut stream) = accept(tcp).await.unwrap().split();
    let HostFrame::Hello { host_id, proof, .. } = read_host_frame(&mut stream).await else {
        panic!("expected hello");
    };
    let key = HostKey::from_seed([1; 32]);
    assert_eq!(proof, key.sign_hello(&[5u8; 32], &host_id, PROTOCOL_VERSION));
}

#[tokio::test]
async fn a_collector_that_sends_no_nonce_gets_no_hello() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let host = tokio::spawn(run(host_with_fake(addr, "no-nonce", slow_fake())));
    let (tcp, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .expect("host connects")
        .unwrap();
    // A plain upgrade, without the nonce header.
    let (_sink, mut stream) = tokio_tungstenite::accept_async(tcp).await.unwrap().split();
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("the host gives up on the connection");
    assert!(
        !matches!(first, Some(Ok(Message::Text(_)))),
        "the host sent a frame without a nonce to sign: {first:?}"
    );
    host.abort();
}
```

- [ ] **Step 5: Update the lock file, regenerate and run the tests**

Run: `cargo build --workspace` then `cargo run -p hennery-proto --bin gen`.
Expected: `wrote …` for both generated files; the TypeScript `HostFrame`'s `hello` member now has `proof: string` and no `token`.

Run: `cargo test -p hennery-testkit --test auth --test host_connection --locked`
Expected: all pass.

- [ ] **Step 6: Revert-probe the per-connection nonce**

In `ws.rs`'s `upgrade`, replace `random_bytes::<32>()` with `[0u8; 32]`, and rerun `cargo test -p hennery-testkit --test auth --locked`. Expected: `a_proof_is_good_on_its_own_connection_only` fails at `assert_ne!(first_nonce, second_nonce)`. Restore the code.

- [ ] **Step 7: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 347 tests pass. `grep -rn 'dev_token\|DevToken' crates/hennery-host crates/hennery-sessions/src/ws.rs` finds nothing.

- [ ] **Step 8: Commit and push**

```bash
git add Cargo.lock crates schema web/src/generated
git commit -m "feat(proto): hello proves the host's key over a per-connection nonce"
git push
```

### Task 6: Revoking a host

**Files:**
- Create: `crates/hennery-kernel/src/lifecycle.rs`
- Modify: `crates/hennery-kernel/src/lib.rs`, `crates/hennery-proto/src/frames.rs` (`HostRevoked`), `crates/hennery-sessions/src/{hub,store,lib,ws,hosts}.rs`
- Regenerate: `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-sessions/tests/store.rs`, `crates/hennery-sessions/tests/hub.rs`, `crates/hennery-testkit/tests/reconcile.rs`

**Interfaces:**
- Consumes: Task 1's `Hosts::{revoke, is_revoked, host}`; Task 5's `host_item`; the store's `collector_event`, `resolve_open_turn`, `cancel_open_pending` as `c6315b2` has them.
- Produces:
  - `PendingReason::HostRevoked` (`host_revoked` on the wire);
  - `hennery_kernel::lifecycle::LifecycleHooks: Send + Sync`, with `fn on_host_revoked(&self, host_id: &str) -> anyhow::Result<()>`, implemented for `AppState`;
  - `Hub::disconnect_and_wait(&self, host_id: &str, bound: Duration) -> bool`, `true` once no connection is left;
  - `Store::revoke_host(&self, host_id: &str) -> Result<Vec<EventDto>>`;
  - `DELETE /api/hosts/{id}` (bearer): 200 `HostItem`, or 404 `not_found`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-sessions/tests/store.rs`:

```rust
// Plan 3a: host revoke (kernel spec §4.3).

#[test]
fn a_revoked_hosts_sessions_are_parked_for_good_and_what_they_held_is_cancelled() {
    let store = Store::open_in_memory().unwrap();
    // s3: presumed parked while its host was away, its turn still open.
    store.create_session("s3", "h1", "fake", "/tmp").unwrap();
    store
        .ingest("s3", 1, &SessionBody::session_started("r3", "a3"))
        .unwrap();
    store.open_turn("s3", "t3", &prompt_text()).unwrap();
    store.ingest("s3", 2, &turn_started("t3")).unwrap();
    store.presume_parked("h1").unwrap();
    // s1: running, with a question whose answer is queued.
    running(&store);
    store.ingest("s1", 3, &permission("p1")).unwrap();
    assert!(matches!(
        store.submit_answer("s1", "p1", &choose("allow")).unwrap(),
        AnswerSubmission::Queued(_)
    ));
    // s2: still starting; s5: active, the operator's close not confirmed;
    // s4: on another host.
    store.create_session("s2", "h1", "fake", "/tmp").unwrap();
    store.create_session("s5", "h1", "fake", "/tmp").unwrap();
    store
        .ingest("s5", 1, &SessionBody::session_started("r5", "a5"))
        .unwrap();
    store.record_close_request("s5").unwrap();
    store.create_session("s4", "h2", "fake", "/tmp").unwrap();
    store
        .ingest("s4", 1, &SessionBody::session_started("r4", "a4"))
        .unwrap();

    let events = store.revoke_host("h1").unwrap();
    assert_eq!(
        kinds(&events),
        [
            "presumed_parked",
            "turn_ended_synthesized",
            "pending_cancelled",
            "presumed_parked",
            "turn_ended_synthesized",
            "presumed_parked"
        ]
    );
    assert_eq!(events[0].body, json!({ "reason": "host_revoked" }));
    for id in ["s1", "s3"] {
        let row = store.session(id).unwrap().unwrap();
        assert_eq!(
            (
                row.lifecycle.as_str(),
                row.presumed_parked,
                row.open_turn_id,
                row.activity
            ),
            ("parked", true, None, None),
            "{id}"
        );
    }
    assert_eq!(
        state_of(&store, "p1"),
        (PendingState::Cancelled, Some(PendingReason::HostRevoked))
    );
    assert_eq!(store.pending_item("p1").unwrap().unwrap().delivered, Some(false));
    let s2 = store.session("s2").unwrap().unwrap();
    assert_eq!(
        (s2.lifecycle.as_str(), s2.failure_reason.as_deref()),
        ("failed", Some("host_revoked"))
    );
    let s5 = store.session("s5").unwrap().unwrap();
    assert_eq!((s5.lifecycle.as_str(), s5.presumed_parked), ("closed", false));
    assert_eq!(store.session("s4").unwrap().unwrap().lifecycle, "active");

    // A repeated revoke finds nothing left to do.
    assert!(store.revoke_host("h1").unwrap().is_empty());
}
```

Append to `crates/hennery-sessions/tests/hub.rs`:

```rust
// Plan 3a: a revoke waits for the host's socket task to let go.

#[tokio::test]
async fn disconnect_and_wait_returns_once_the_socket_task_has_let_go() {
    let hub = Arc::new(Hub::new());
    assert!(
        hub.disconnect_and_wait("h", Duration::from_millis(10)).await,
        "no connection: nothing to wait for"
    );
    let (registration, _rx) = connect(&hub);
    // The socket task: it notices the kick, and unregisters a little later.
    let socket_task = tokio::spawn({
        let hub = hub.clone();
        async move {
            registration.kicked.cancelled().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            hub.unregister("h", registration.conn_id);
        }
    });
    assert!(hub.disconnect_and_wait("h", Duration::from_secs(10)).await);
    assert!(!hub.is_ready("h"));
    socket_task.await.unwrap();
}

#[tokio::test]
async fn disconnect_and_wait_gives_up_after_its_bound() {
    let hub = Hub::new();
    let (registration, _rx) = connect(&hub);
    assert!(!hub.disconnect_and_wait("h", Duration::from_millis(50)).await);
    assert!(registration.kicked.is_cancelled());
}
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host
    }
```

with:

```rust
        assert!(matches!(ack, CollectorFrame::HelloAck { .. }), "{ack:?}");
        host
    }

    /// Connect and send `hello` (no sessions attached): the collector's
    /// answer, whatever it is.
    async fn hello_reply(collector: &Collector) -> CollectorFrame {
        let (ws, response) = tokio_tungstenite::connect_async(format!("ws://{}/api/hosts/ws", collector.addr))
            .await
            .unwrap();
        let nonce = hex::decode(response.headers()[HELLO_NONCE_HEADER].to_str().unwrap()).unwrap();
        let mut host = Self { ws, seq: 0 };
        host.send(&HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: "test".into(),
            host_id: HOST.into(),
            proof: host_key().sign_hello(&nonce, HOST, PROTOCOL_VERSION),
            capabilities: Capabilities::default(),
            attached_sessions: vec![],
        })
        .await;
        host.next().await
    }

    /// Wait until the collector has closed this connection.
    async fn closed(mut self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(Ok(msg)) = self.ws.next().await {
                if matches!(msg, Message::Close(_)) {
                    break;
                }
            }
        })
        .await
        .expect("the collector closes the connection");
    }
```

In `crates/hennery-testkit/tests/reconcile.rs`, replace:

```rust
        assert!(changed.contains(r#""pending_id":"p1""#), "{changed}");
    }
}
```

with:

```rust
        assert!(changed.contains(r#""pending_id":"p1""#), "{changed}");
    }
}

// Plan 3a: host revoke (kernel spec §4.3).

async fn revoke(collector: &Collector, host_id: &str) -> (u16, Value) {
    let resp = client()
        .delete(collector.url(&format!("/api/hosts/{host_id}")))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_revoke_closes_the_hosts_connection_and_parks_its_sessions_for_good() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let (session, _) = asking_session(&collector, &mut host).await;
    // An answer the host has, with no verdict yet.
    let (status, _) = post(
        &client(),
        answer_url(&collector, &session, "p1"),
        json!({"option_id": "allow"}),
    )
    .await;
    assert_eq!(status, 202);
    expect_answer(&mut host, &session, "allow").await;

    let (status, body) = revoke(&collector, HOST).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (body["host_id"].as_str(), body["connected"].as_bool()),
        (Some(HOST), Some(false))
    );
    assert!(body["revoked_at"].is_string(), "{body}");
    host.closed().await;

    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!(
        (row.lifecycle.as_str(), row.presumed_parked, row.open_turn_id),
        ("parked", true, None)
    );
    let kinds = collector.event_kinds(&session);
    assert!(
        kinds.ends_with(&[
            "presumed_parked".to_string(),
            "turn_ended_synthesized".to_string(),
            "pending_cancelled".to_string()
        ]),
        "{kinds:?}"
    );
    let item = collector.state.store.pending_item("p1").unwrap().unwrap();
    assert_eq!(
        serde_json::to_value((item.state, item.reason, item.delivered)).unwrap(),
        json!(["cancelled", "host_revoked", false])
    );
    // Refused from now on, and told why.
    let reply = ScriptedHost::hello_reply(&collector).await;
    assert!(
        matches!(&reply, CollectorFrame::HelloError { code, .. } if code == "revoked"),
        "{reply:?}"
    );
    // A repeated revoke changes nothing more; an unknown host is 404.
    let before = collector.event_kinds(&session).len();
    assert_eq!(revoke(&collector, HOST).await.0, 200);
    assert_eq!(collector.event_kinds(&session).len(), before);
    assert_eq!(revoke(&collector, "host-9").await.0, 404);
}

/// A revoke that interrupts a handshake: the connection is closed before
/// the session is parked, and a kicked connection reads nothing more, so
/// the `resend_complete` the host sends afterwards never reconciles the
/// session back to `active` (`reattached`).
#[tokio::test]
async fn a_revoke_during_a_handshake_is_not_undone_by_its_reconciliation() {
    let collector = Collector::start().await;
    let mut host = ScriptedHost::connect(&collector, vec![], 0).await;
    let session = started_session(&collector, &mut host).await;
    let seq = host.seq;
    host.drop_connection(&collector).await;
    // Back, still running the session, its resend not complete yet.
    let mut host = ScriptedHost::hello(&collector, vec![attached(&session, seq)], seq).await;
    assert_eq!(revoke(&collector, HOST).await.0, 200);
    let _ = host
        .ws
        .send(Message::text(
            serde_json::to_string(&HostFrame::ResendComplete).unwrap(),
        ))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
    assert!(!collector.event_kinds(&session).contains(&"reattached".to_string()));
    assert!(collector.state.hub.connected_hosts().is_empty());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test -p hennery-sessions --test store revoked`
Expected: FAIL to compile. `error[E0599]: no method named revoke_host found for struct Store in the current scope` and `error[E0599]: no variant, associated function, or constant named HostRevoked found for enum PendingReason`.

- [ ] **Step 3: The reason, the hooks, the hub, the store and the endpoint**

In `crates/hennery-proto/src/frames.rs`, replace:

```rust
    AgentWithdrew,
```

with:

```rust
    AgentWithdrew,
    /// The operator revoked the session's host (kernel spec §4.3): it will
    /// never connect again to deliver an answer.
    HostRevoked,
```

Create `crates/hennery-kernel/src/lifecycle.rs`:

```rust
//! Lifecycle hooks (kernel spec §5.5): what the kernel's own operations
//! require of the modules that own session and gateway state. They
//! implement the trait, so the kernel never imports them.

/// Called by the kernel's lifecycle operations once the kernel's own part
/// is done.
pub trait LifecycleHooks: Send + Sync {
    /// The host is revoked and its connection is gone (kernel spec §4.3):
    /// nothing it held will ever be reconciled. Must be idempotent: a
    /// repeated revoke calls it again.
    fn on_host_revoked(&self, host_id: &str) -> anyhow::Result<()>;
}
```

In `crates/hennery-kernel/src/lib.rs`, replace:

```rust
pub mod hosts;
```

with:

```rust
pub mod hosts;
pub mod lifecycle;
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
    kicked: CancellationToken,
    /// From this connection's `hello` (ACP core §3.3).
```

with:

```rust
    kicked: CancellationToken,
    /// Cancelled once this connection is unregistered (or replaced).
    ended: CancellationToken,
    /// From this connection's `hello` (ACP core §3.3).
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        hosts.insert(
```

with:

```rust
        let replaced = hosts.insert(
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
                kicked: kicked.clone(),
```

with:

```rust
                kicked: kicked.clone(),
                ended: CancellationToken::new(),
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        );
        self.last_conn
```

with:

```rust
        );
        if let Some(old) = replaced {
            old.ended.cancel();
        }
        self.last_conn
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        if hosts.get(host_id).is_some_and(|h| h.conn_id == conn_id) {
            hosts.remove(host_id);
```

with:

```rust
        if hosts.get(host_id).is_some_and(|h| h.conn_id == conn_id)
            && let Some(gone) = hosts.remove(host_id)
        {
            gone.ended.cancel();
```

In `crates/hennery-sessions/src/hub.rs`, replace:

```rust
        if let Some(h) = self.hosts.lock().expect("hosts lock").get(host_id) {
            h.kicked.cancel();
        }
    }

    /// Like `disconnect`, but only if `conn_id` is still the host's current
```

with:

```rust
        if let Some(h) = self.hosts.lock().expect("hosts lock").get(host_id) {
            h.kicked.cancel();
        }
    }

    /// Close the host's connection and wait, at most `bound`, until its
    /// socket task has unregistered it: then nothing that connection reads
    /// can change the store any more. `true` once no connection is left.
    pub async fn disconnect_and_wait(&self, host_id: &str, bound: Duration) -> bool {
        let ended = {
            let hosts = self.hosts.lock().expect("hosts lock");
            let Some(h) = hosts.get(host_id) else {
                return true;
            };
            h.kicked.cancel();
            h.ended.clone()
        };
        tokio::time::timeout(bound, ended.cancelled()).await.is_ok()
    }

    /// Like `disconnect`, but only if `conn_id` is still the host's current
```

In `crates/hennery-sessions/src/store.rs`, replace:

```rust
    }

    /// Hosts the collector believes are running at least one session.
```

with:

```rust
    }

    /// The host was revoked (kernel spec §4.3). It never connects again, so
    /// nothing will ever reconcile its sessions:
    /// - a start or resume still in flight fails `host_revoked`;
    /// - every session it ran (`active`, or presumed parked while it was
    ///   away) is presumed parked for good (`presumed_parked{host_revoked}`),
    ///   its open turn ends (`interrupted` if the adapter had it) and its
    ///   open questions are cancelled `host_revoked`, which also gives any
    ///   queued answer its `delivered: false`;
    /// - one the operator had asked to close is closed instead.
    ///
    /// Idempotent: a session already presumed parked for the revocation is
    /// left as it is.
    pub fn revoke_host(&self, host_id: &str) -> Result<Vec<EventDto>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let ts = now();
        tx.execute(
            "UPDATE sessions SET lifecycle = 'failed', failure_reason = 'host_revoked'
             WHERE host_id = ?1 AND lifecycle = 'starting'",
            [host_id],
        )?;
        let rows: Vec<(String, Option<String>)> = {
            let mut stmt = tx.prepare(
                "SELECT id, open_turn_id FROM sessions
                 WHERE host_id = ?1 AND (lifecycle = 'active' OR presumed_parked = 1) ORDER BY id",
            )?;
            let rows = stmt.query_map([host_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut events = Vec::new();
        for (id, open_turn) in rows {
            let presumed_for: Option<Option<String>> = tx
                .query_row(
                    "SELECT json_extract(body, '$.reason') FROM events
                     WHERE session_id = ?1 AND kind = 'presumed_parked' ORDER BY event_id DESC LIMIT 1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()?;
            if presumed_for.flatten().as_deref() == Some("host_revoked") {
                continue;
            }
            events.push(collector_event(
                &tx,
                &id,
                "presumed_parked",
                json!({ "reason": "host_revoked" }),
                &ts,
            )?);
            if let Some(turn) = open_turn {
                events.push(resolve_open_turn(&tx, &id, &turn, &ts)?);
            }
            events.extend(cancel_open_pending(&tx, &id, PendingReason::HostRevoked, &ts)?);
            tx.execute(
                "UPDATE sessions SET
                     lifecycle = CASE WHEN close_requested = 1 THEN 'closed' ELSE 'parked' END,
                     presumed_parked = CASE WHEN close_requested = 1 THEN 0 ELSE 1 END,
                     activity = NULL, open_turn_id = NULL, close_requested = 0
                 WHERE id = ?1",
                [&id],
            )?;
        }
        tx.commit()?;
        Ok(events)
    }

    /// Hosts the collector believes are running at least one session.
```

In `crates/hennery-sessions/src/lib.rs`, replace:

```rust
}

/// Serve until `state.shutdown` is cancelled.
```

with:

```rust
}

/// The session module's part of a host revoke (kernel spec §4.3, §5.5).
/// The revoke endpoint calls it once the host's connection is gone.
impl hennery_kernel::lifecycle::LifecycleHooks for AppState {
    fn on_host_revoked(&self, host_id: &str) -> anyhow::Result<()> {
        for event in self.store.revoke_host(host_id)? {
            self.hub.publish(event);
        }
        Ok(())
    }
}

/// Serve until `state.shutdown` is cancelled.
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        return;
    };

    let mut committed = BTreeMap::new();
```

with:

```rust
        return;
    };
    // A revoke that landed after the proof was checked but before this
    // registration found no connection to close: this one must not live
    // on to reconcile what the revoke parked (kernel spec §4.3).
    if !matches!(state.hosts.is_revoked(&host_id), Ok(false)) {
        state.hub.unregister(&host_id, registration.conn_id);
        let _ = sink
            .send(text(&reject(
                "revoked",
                "this host was revoked; pair it again with `hennery host join`",
            )))
            .await;
        return;
    }

    let mut committed = BTreeMap::new();
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
    loop {
        let next = tokio::select! {
```

with:

```rust
    loop {
        // Biased: a kicked connection (a timed-out request, a revoke) reads
        // no further frame, however many are waiting.
        let next = tokio::select! {
```

In `crates/hennery-sessions/src/ws.rs`, replace:

```rust
        let next = tokio::select! {
```

with:

```rust
        let next = tokio::select! {
            biased;
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
//! Host pairing endpoints (kernel spec §4.1, §8): minting pairing codes and
//! enrollment. They live beside the session API because the collector's
```

with:

```rust
//! Host endpoints (kernel spec §4, §8): the host list, minting pairing
//! codes, enrollment and revoke. They live beside the session API because the collector's
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use axum::extract::{ConnectInfo, State};
```

with:

```rust
use axum::extract::{ConnectInfo, Path, State};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use axum::routing::{get, post};
```

with:

```rust
use axum::routing::{delete, get, post};
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord};
```

with:

```rust
use hennery_kernel::hosts::{EnrollOutcome, Enrollment, HostRecord, Revoke};
use hennery_kernel::lifecycle::LifecycleHooks;
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
use std::time::Instant;
```

with:

```rust
use std::time::{Duration, Instant};

/// How long a revoke waits for the host's socket task to let go.
const REVOKE_DISCONNECT_BOUND: Duration = Duration::from_secs(10);
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
        .route("/api/hosts/pairing-codes", post(mint_pairing_code))
```

with:

```rust
        .route("/api/hosts/pairing-codes", post(mint_pairing_code))
        .route("/api/hosts/{id}", delete(revoke_host))
```

In `crates/hennery-sessions/src/hosts.rs`, replace:

```rust
        Ok(EnrollOutcome::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
```

with:

```rust
        Ok(EnrollOutcome::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}

/// `DELETE /api/hosts/{id}`: revoke a host (kernel spec §4.3), 200 with its
/// entry. In this order: the registry refuses its `hello`s from now on,
/// its live connection is closed and gone, and only then are its sessions
/// parked, so no reconciliation on that connection can bring them back.
/// Repeating it repeats the steps, which heals a revoke cut short.
async fn revoke_host(State(state): State<AppState>, Path(host_id): Path<String>) -> Response {
    match state.hosts.revoke(&host_id, unix_now()) {
        Ok(Revoke::NotFound) => return error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Ok(Revoke::Revoked | Revoke::AlreadyRevoked) => {}
        Err(err) => return internal(err),
    }
    if !state.hub.disconnect_and_wait(&host_id, REVOKE_DISCONNECT_BOUND).await {
        tracing::warn!(%host_id, "the revoked host's connection did not close in time");
    }
    if let Err(err) = state.on_host_revoked(&host_id) {
        return internal(err);
    }
    tracing::info!(%host_id, "host revoked");
    match state.hosts.host(&host_id) {
        Ok(Some(record)) => Json(host_item(&state, record)).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not_found", "no such host"),
        Err(err) => internal(err),
    }
}
```

- [ ] **Step 4: Regenerate and run the tests**

Run: `cargo run -p hennery-proto --bin gen`
Expected: `PendingReason` in the TypeScript ends with `| "host_revoked"`.

Run: `cargo test -p hennery-sessions --test store --test hub --locked && cargo test -p hennery-testkit --test reconcile revoke --locked`
Expected: all pass.

- [ ] **Step 5: Revert-probe the hook**

In `revoke_host` (`hosts.rs`), comment out the `on_host_revoked` call, and rerun the reconcile tests above. Expected: `a_revoke_closes_the_hosts_connection_and_parks_its_sessions_for_good` fails on the lifecycle (`active`, not `parked`). Restore the code.

- [ ] **Step 6: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 352 tests pass.

- [ ] **Step 7: Commit and push**

```bash
git add crates schema web/src/generated
git commit -m "feat(sessions): revoke a host, closing its connection and parking its sessions for good"
git push
```

### Task 7: A revoked host stops, and `join` asks the collector

**Files:**
- Modify: `crates/hennery-host/src/connection.rs` (`HelloRejected`, `revoked`, `Standing`, `probe`, `handshake`, `run_until`), `crates/hennery-host/src/outbox.rs` (`FILE`), `crates/hennery-host/src/pairing.rs` (`join`)
- Test: `crates/hennery-testkit/tests/e2e.rs`, `crates/hennery-testkit/tests/join.rs`

**Interfaces:**
- Consumes: Task 5's signed handshake; Task 6's revoke endpoint.
- Produces (`hennery_host::connection`):
  - `struct HelloRejected { code, message: String }` (`std::error::Error`);
  - `fn revoked(&anyhow::Error) -> bool`;
  - `enum Standing { Accepted, Revoked, Unknown }`;
  - `async fn probe(collector_url: &str, host_id: &str, key: &HostKey) -> Result<Standing>`.
- Produces: `run_until` now returns `Err` (context "this host was revoked; …") once a `hello` is refused `revoked`, after stopping every adapter. `run`, which never returned before, returns that error.
- Produces: `hennery_host::outbox::FILE = "outbox.db"`.
- Changes: `pairing::join` probes an existing pairing, re-pairs a revoked or unknown one (dropping the outbox), and refuses one for another collector.

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-testkit/tests/e2e.rs`:

```rust
// Plan 3a: a revoked host stops its adapters (ACP core §3.5, kernel spec §4.3).

#[tokio::test]
async fn a_revoked_host_stops_its_adapters_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let collector = Collector::start(&dir.path().join("hennery.db"), None).await;
    let pid_file = dir.path().join("grandchild.pid");
    let script = FakeScript {
        grandchild_pid_file: Some(pid_file.to_string_lossy().into_owned()),
        ..slow_script(20)
    };
    let mut fake = AgentCommand::parse(env!("CARGO_BIN_EXE_hennery-fake-acp")).unwrap();
    fake.env
        .push((SCRIPT_ENV.into(), serde_json::to_string(&script).unwrap()));
    let host = tokio::spawn(hennery_host::run(host_config(
        collector.addr,
        &dir.path().join("host"),
        fake,
    )));
    let c = client();
    wait_host_connected(&c, &collector).await;
    let session = start_session(&c, &collector).await;
    let grandchild = wait_for("grandchild pid", || async { pid_from(&pid_file) }).await;

    let revoked = c.delete(collector.url("/api/hosts/host-1")).send().await.unwrap();
    assert_eq!(revoked.status(), 200);
    // Kicked, the host reconnects, is told it is revoked, and stops.
    let outcome = tokio::time::timeout(Duration::from_secs(30), host)
        .await
        .expect("the revoked host stops")
        .unwrap();
    let err = outcome.expect_err("a revoked host ends with an error");
    assert!(format!("{err:#}").contains("revoked"), "{err:#}");
    wait_dead(grandchild).await;
    let row = collector.state.store.session(&session).unwrap().unwrap();
    assert_eq!((row.lifecycle.as_str(), row.presumed_parked), ("parked", true));
}
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
//! that is paired already is left alone.
```

with:

```rust
//! the collector still accepts is left alone, and one it revoked pairs anew.
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
//! the collector still accepts is left alone, and one it revoked pairs anew.

use hennery_host::identity::{KEY_FILE, Paired};
```

with:

```rust
//! the collector still accepts is left alone, and one it revoked pairs anew.

use hennery_host::HostConfig;
use hennery_host::identity::{KEY_FILE, Paired};
```

In `crates/hennery-testkit/tests/join.rs`, replace:

```rust
    assert!(collector.state.hosts.list().unwrap().is_empty());
}
```

with:

```rust
    assert!(collector.state.hosts.list().unwrap().is_empty());
}

async fn joined(collector: &Collector, dir: &std::path::Path) -> String {
    let code = collector.mint().await;
    match join(&collector.public_url(), &code, dir, "laptop").await.unwrap() {
        Joined::Paired { host_id } => host_id,
        other => panic!("expected a new pairing, got {other:?}"),
    }
}

#[tokio::test]
async fn joining_again_while_the_host_runs_leaves_it_running() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let host_id = joined(&collector, dir.path()).await;
    let paired = Paired::load(dir.path()).unwrap().unwrap();
    let host = tokio::spawn(hennery_host::run(HostConfig::new(
        paired.collector_url,
        paired.host_id,
        paired.key,
        dir.path().to_path_buf(),
    )));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !collector.state.hub.is_ready(&host_id) {
        assert!(tokio::time::Instant::now() < deadline, "host never connected");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // The probe is refused `already_connected`, which the collector says
    // only of a valid proof: still paired.
    let code = collector.mint().await;
    assert_eq!(
        join(&collector.public_url(), &code, dir.path(), "laptop")
            .await
            .unwrap(),
        Joined::AlreadyPaired {
            host_id: host_id.clone()
        }
    );
    assert!(collector.state.hub.is_ready(&host_id));
    host.abort();
}

#[tokio::test]
async fn joining_again_after_a_revoke_pairs_anew_and_drops_the_old_outbox() {
    let collector = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    let old_id = joined(&collector, dir.path()).await;
    let old_key = std::fs::read(dir.path().join(KEY_FILE)).unwrap();
    std::fs::write(dir.path().join("outbox.db"), b"frames of the old identity").unwrap();
    collector
        .state
        .hosts
        .revoke(&old_id, hennery_kernel::secret::unix_now())
        .unwrap();

    let new_id = joined(&collector, dir.path()).await;
    assert_ne!(new_id, old_id);
    assert_ne!(std::fs::read(dir.path().join(KEY_FILE)).unwrap(), old_key);
    assert!(!dir.path().join("outbox.db").exists());
    assert_eq!(Paired::load(dir.path()).unwrap().unwrap().host_id, new_id);
    let hosts = collector.state.hosts.list().unwrap();
    assert_eq!(hosts.len(), 2);
    assert!(hosts.iter().any(|h| h.id == old_id && h.revoked_at.is_some()));
}

#[tokio::test]
async fn joining_another_collector_while_paired_is_refused() {
    let first = Collector::start().await;
    let second = Collector::start().await;
    let dir = tempfile::tempdir().unwrap();
    joined(&first, dir.path()).await;
    let code = second.mint().await;
    let err = join(&second.public_url(), &code, dir.path(), "laptop")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is paired with"), "{err}");
    assert!(second.state.hosts.list().unwrap().is_empty());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --no-fail-fast -p hennery-testkit --test join --test e2e -- joining a_revoked_host`
Expected: FAIL. `a_revoked_host_stops_its_adapters_and_exits` fails after 30 s at "the revoked host stops" (the host keeps reconnecting). `joining_again_after_a_revoke_pairs_anew_and_drops_the_old_outbox` panics "expected a new pairing, got AlreadyPaired", and `joining_another_collector_while_paired_is_refused` at its `unwrap_err`: `join` still trusts the files. `joining_again_while_the_host_runs_leaves_it_running` already passes, for the same reason.

- [ ] **Step 3: Stop on `revoked`, and probe before re-joining**

In `crates/hennery-host/src/outbox.rs`, replace:

```rust
use std::path::Path;
```

with:

```rust
use std::path::Path;

/// The outbox's file in the host's data directory.
pub const FILE: &str = "outbox.db";
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let outbox = Outbox::open(&cfg.data_dir.join("outbox.db"))?;
```

with:

```rust
    let outbox = Outbox::open(&cfg.data_dir.join(crate::outbox::FILE))?;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
```

with:

```rust
    let sessions: Sessions = Arc::new(Mutex::new(SessionMap::default()));
    // Ends only when the collector says this host is revoked.
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            if let Err(err) = connect_once(&cfg, &uplink, &sessions, &mut replies, &mut backoff).await {
```

with:

```rust
            if let Err(err) = connect_once(&cfg, &uplink, &sessions, &mut replies, &mut backoff).await {
                if revoked(&err) {
                    return err;
                }
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    tokio::select! {
        _ = serve => unreachable!("the connection loop never ends"),
        _ = shutdown => {}
```

with:

```rust
    let revocation = tokio::select! {
        err = serve => Some(err),
        _ = shutdown => None,
    };
    // A revoked host stops every adapter it runs (ACP core §3.5), the same
    // way a shutdown does.
    shut_down(&sessions, cfg.session_options().kill_grace + Duration::from_secs(1)).await;
    match revocation {
        Some(err) => Err(err.context("this host was revoked; pair it again with `hennery host join`")),
        None => Ok(()),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    shut_down(&sessions, cfg.session_options().kill_grace + Duration::from_secs(1)).await;
    Ok(())
```

with:

```rust
}

/// A `hello_error` (ACP core §3.3): the collector refused this host's
/// `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloRejected {
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for HelloRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hello rejected: {}: {}", self.code, self.message)
    }
}

impl std::error::Error for HelloRejected {}

/// Whether `err` is the collector saying this host is revoked.
pub fn revoked(err: &anyhow::Error) -> bool {
    err.downcast_ref::<HelloRejected>().is_some_and(|r| r.code == "revoked")
}

/// What the collector makes of a stored pairing (`hennery host join`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// It accepts the key (the host may be connected right now).
    Accepted,
    Revoked,
    /// It does not know this host, or not with this key.
    Unknown,
}

/// Bound on each step of a probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Ask the collector whether it still accepts this host's key: one `hello`
/// with nothing attached, then the connection is closed.
pub async fn probe(collector_url: &str, host_id: &str, key: &HostKey) -> Result<Standing> {
    let (mut sink, _stream, answer) = handshake(
        collector_url,
        host_id,
        key,
        || Ok(Vec::new()),
        PROBE_TIMEOUT,
        PROBE_TIMEOUT,
    )
    .await?;
    let _ = sink.close().await;
    Ok(match answer {
        CollectorFrame::HelloAck { .. } => Standing::Accepted,
        // Refused only after the proof checked out (ACP core §3.5).
        CollectorFrame::HelloError { code, .. } if code == "already_connected" => Standing::Accepted,
        CollectorFrame::HelloError { code, .. } if code == "revoked" => Standing::Revoked,
        CollectorFrame::HelloError { code, .. } if code == "bad_proof" => Standing::Unknown,
        CollectorFrame::HelloError { code, message } => return Err(HelloRejected { code, message }.into()),
        other => bail!("expected hello_ack, got {other:?}"),
    })
}

type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Connect, send a `hello` signed over this connection's nonce (ACP core
/// §3.5), and read the collector's answer. `attached` is read once the
/// socket is up, right before the `hello` goes out.
async fn handshake(
    collector_url: &str,
    host_id: &str,
    key: &HostKey,
    attached: impl FnOnce() -> Result<Vec<AttachedSession>>,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<(WsSink, WsStream, CollectorFrame)> {
    let (ws, response) = tokio::time::timeout(connect_timeout, tokio_tungstenite::connect_async(collector_url))
        .await
        .map_err(|_| anyhow::anyhow!("no WebSocket handshake within {connect_timeout:?}"))?
        .context("connect to collector")?;
    // The proof is over this connection's nonce, so it cannot be replayed on
    // another one. Without one there is nothing to sign.
    let nonce = response
        .headers()
        .get(HELLO_NONCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| hex::decode(v).ok())
        .filter(|n| n.len() == 32)
        .context("the collector sent no hello nonce")?;
    let (mut sink, mut stream) = ws.split();
    send(
        &mut sink,
        &HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            host_id: host_id.to_string(),
            proof: key.sign_hello(&nonce, host_id, PROTOCOL_VERSION),
            // Every hennery host can park. `projects` and `images` come
            // with the probes and with image prompts.
            capabilities: Capabilities(vec![Capability::Park]),
            attached_sessions: attached()?,
        },
    )
    .await?;
    let answer = match tokio::time::timeout(read_timeout, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<CollectorFrame>(&text)?,
        other => bail!("no hello_ack: {other:?}"),
    };
    Ok((sink, stream, answer))
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
) -> Result<()> {
    let (ws, response) = tokio::time::timeout(
```

with:

```rust
) -> Result<()> {
    let (mut sink, mut stream, answer) = handshake(
        &cfg.collector_url,
        &cfg.host_id,
        &cfg.key,
        || attached_sessions(uplink, sessions),
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
        tokio_tungstenite::connect_async(&cfg.collector_url),
    )
    .await
    .map_err(|_| anyhow::anyhow!("no WebSocket handshake within {:?}", cfg.connect_timeout))?
    .context("connect to collector")?;
    // The proof is over this connection's nonce, so it cannot be replayed on
    // another one (ACP core §3.5). Without one there is nothing to sign.
    let nonce = response
        .headers()
        .get(HELLO_NONCE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| hex::decode(v).ok())
        .filter(|n| n.len() == 32)
        .context("the collector sent no hello nonce")?;
    let (mut sink, mut stream) = ws.split();

    let attached = attached_sessions(uplink, sessions)?;
    send(
        &mut sink,
        &HostFrame::Hello {
            protocol_version: PROTOCOL_VERSION.into(),
            host_version: env!("CARGO_PKG_VERSION").into(),
            host_id: cfg.host_id.clone(),
            proof: cfg.key.sign_hello(&nonce, &cfg.host_id, PROTOCOL_VERSION),
            // Every hennery host can park. `projects` and `images` come
            // with the probes and with image prompts.
            capabilities: Capabilities(vec![Capability::Park]),
            attached_sessions: attached,
        },
```

with:

```rust
        cfg.read_timeout,
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
    .await?;

    match tokio::time::timeout(cfg.read_timeout, stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<CollectorFrame>(&text)? {
            CollectorFrame::HelloAck { committed, .. } => {
                for (session_id, seq) in committed {
                    uplink.fast_forward(&session_id, seq)?;
                }
```

with:

```rust
    .await?;
    match answer {
        CollectorFrame::HelloAck { committed, .. } => {
            for (session_id, seq) in committed {
                uplink.fast_forward(&session_id, seq)?;
```

In `crates/hennery-host/src/connection.rs`, replace:

```rust
            CollectorFrame::HelloError { code, message } => bail!("hello rejected: {code}: {message}"),
            other => bail!("expected hello_ack, got {other:?}"),
        },
        other => bail!("no hello_ack: {other:?}"),
```

with:

```rust
        }
        CollectorFrame::HelloError { code, message } => return Err(HelloRejected { code, message }.into()),
        other => bail!("expected hello_ack, got {other:?}"),
```

In `crates/hennery-host/src/pairing.rs`, replace:

```rust
//! key, enroll it with a pairing code, and store the pairing.

use crate::identity::{HostKey, KEY_FILE, Paired};
```

with:

```rust
//! key, enroll it with a pairing code, and store the pairing.

use crate::connection::{Standing, probe};
use crate::identity::{HostKey, KEY_FILE, Paired};
```

In `crates/hennery-host/src/pairing.rs`, replace:

```rust
use crate::identity::{HostKey, KEY_FILE, Paired};
```

with:

```rust
use crate::identity::{HostKey, KEY_FILE, Paired};
use crate::outbox::FILE as OUTBOX_FILE;
```

In `crates/hennery-host/src/pairing.rs`, replace:

```rust
    /// The data directory holds a pairing already; nothing was sent.
```

with:

```rust
    /// The collector still accepts the stored pairing; the code was not
    /// used.
```

In `crates/hennery-host/src/pairing.rs`, replace:

```rust
/// `public_url`. Idempotent: a directory that is paired already is left as
/// it is, and the code is not spent.
```

with:

```rust
/// `public_url` (kernel spec §4.1). Idempotent: if the collector still
/// accepts the stored key, nothing changes and the code is not spent. A
/// pairing it revoked or no longer knows is replaced by a new key and a new
/// host id, and the outbox of the old identity is dropped: its sessions
/// belong to a host id the collector will never hear from again.
```

In `crates/hennery-host/src/pairing.rs`, replace:

```rust
pub async fn join(public_url: &str, code: &str, data_dir: &Path, name: &str) -> Result<Joined> {
    if let Some(paired) = Paired::load(data_dir)? {
        return Ok(Joined::AlreadyPaired {
            host_id: paired.host_id,
        });
    }
```

with:

```rust
pub async fn join(public_url: &str, code: &str, data_dir: &Path, name: &str) -> Result<Joined> {
```

In `crates/hennery-host/src/pairing.rs`, replace:

```rust
    let collector_url = collector_ws_url(public_url)?;
```

with:

```rust
    let collector_url = collector_ws_url(public_url)?;
    if let Some(paired) = Paired::load(data_dir)? {
        if paired.collector_url != collector_url {
            bail!(
                "{} is paired with {} already; remove its {KEY_FILE} and host.toml to pair it with {public_url} instead",
                data_dir.display(),
                paired.collector_url
            );
        }
        match probe(&collector_url, &paired.host_id, &paired.key).await? {
            Standing::Accepted => {
                return Ok(Joined::AlreadyPaired {
                    host_id: paired.host_id,
                });
            }
            Standing::Revoked | Standing::Unknown => {
                for file in [
                    OUTBOX_FILE.to_string(),
                    format!("{OUTBOX_FILE}-wal"),
                    format!("{OUTBOX_FILE}-shm"),
                ] {
                    let _ = std::fs::remove_file(data_dir.join(file));
                }
            }
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p hennery-testkit --test join --test e2e --test host_connection --locked`
Expected: all pass; `a_revoked_host_stops_its_adapters_and_exits` in about a second.

- [ ] **Step 5: Revert-probe the stop**

In `run_until`, change `if revoked(&err) {` to `if false && revoked(&err) {`, and rerun `cargo test -p hennery-testkit --test e2e a_revoked_host --locked`. Expected: it fails after 30 s at "the revoked host stops". Restore the code.

- [ ] **Step 6: Check the timing-sensitive tests under load**

Run four copies of each binary at once: `for b in e2e reconcile join host_connection; do for i in 1 2 3 4; do cargo test -p hennery-testkit --test $b --locked -q & done; wait; done`, then the same for `-p hennery-sessions --test hub` and `-p hennery --test cli`.
Expected: every copy passes.

- [ ] **Step 7: Run the whole gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo clippy -p hennery --locked -- -D warnings && cargo test --workspace --locked && cargo run -p hennery-proto --bin gen -- --check`
Expected: all 356 tests pass.

- [ ] **Step 8: Commit and push**

```bash
git add crates/hennery-host crates/hennery-testkit
git commit -m "feat(host): a revoked host stops its adapters; join re-pairs a revoked host"
git push
```

## After this plan

**Plan 3b, operator auth** (kernel §3, §4.2, §7; umbrella §7):
- **Setup.** The one-time setup link: 256 bits, 1 h, single use, printed only to a TTY, otherwise written to `setup-url` 0600. The owner password (Argon2id via `password-auth`, verified on a blocking thread). `public_url` (`https://` or loopback). The default hat's name, to be stored once hats exist.
- **The `owners` table, with `owner_id` on every kernel and session table** (decision 2).
- **Login and sessions.**
  - `POST /api/auth/login` / `logout`, rate limited per address with Task 2's `Limiter` (`Policy { 5 per minute, … }`) and a constant-time failure path.
  - `auth_sessions`: a random 256-bit id, hashed at rest, 30-day sliding expiry.
  - The cookie `hennery_session`: `HttpOnly`, `Secure` except on loopback, `SameSite=Strict`, `Path=/`.
  - `GET/DELETE /api/auth/sessions`.
- **Browser routes.** The `Origin` / `Sec-Fetch-Site` rules per route class (§3.3). JSON only on state-changing routes. Enrollment, the host WebSocket and the health checks stay exempt; tests on every listener.
- **Step-up** (5 min, `last_step_up_at`): on `POST /api/hosts/pairing-codes` and `DELETE /api/hosts/{id}` now, and on every later action in §3.4. The 403 is `step_up_required`.
- **Several listeners** (maintainer decision 6c). `listen = […]`, a repeatable `--listen`, and `HENNERY_LISTEN`. Every listener gets the same router and `ConnectInfo`. Start fails if any address is taken; browser access stays bound to `public_url`.
- **`config.toml`** and the precedence of flags, environment and file (§2).
- **The development bearer comes off every production route.** Replace `DevToken`/`require_bearer` with the session cookie. Keep a test-only path, behind the `test-hooks`-style feature, only if the testkit still needs it.
- **The admin socket** (`admin.sock` 0600): print the setup URL, reset the password, list hosts, mint a pairing code. Destructive commands need TTY confirmation.

**Plan 3c, passkeys:**
- `webauthn-rs`, with the RP id and origin from `public_url` and ceremony state in memory.
- Several passkeys, each labelled; removable while another login method remains; step-up by passkey.
- Tests with the `passkey` crate's software authenticator.
- **`webauthn-rs` needs OpenSSL.** Add `openssl` and `pkg-config` to `flake.nix`'s dev shell (no global install). The musl release build needs it vendored and static (distribution §1.1).

**Obligations plan 3a hands on:**
- **`wss://` for remote hosts** (decision 10): enable `tokio-tungstenite`'s rustls feature (with the ring provider reqwest already pulls in), and test it live behind a TLS terminator. `host join https://…` works today, but the host then cannot connect.
- **`PATCH /api/hosts/{id}`** (rename, default hat), with hats.
- **`hello.agents`, `workspace_roots` and `probe_agents`**, and storing them from `hello` (kernel §4.3).
- **A revoked host's sessions.** They stay presumed parked under the dead host id for good, and a resume answers 409 `host_offline`. The frontend should say "host revoked" (the `presumed_parked` event's reason) and offer delete (§4.10) once it exists.
- **`doctor`:** "revoked → re-pair with `hennery host join`" and "unknown host (`bad_proof`) → re-pair" (distribution §7, check 7). A host refused `bad_proof` keeps reconnecting at its backoff until then.
- **The gateway:** implement `LifecycleHooks::on_host_revoked` (`SessionMcp::revoke` for every session of the host), and call every registered hook from the revoke endpoint.
- **Hats:** `LifecycleHooks::on_hat_purged`.
- **The collector's single writer thread** (kernel §1), now that two stores share `hennery.db`.
- **`host.lock`** (distribution §8), so a second `hennery host run` on one directory refuses to start. The probe cannot stand in for it.
- **The supervisor's restart policy** (distribution §5.2): a revoked all-in-one host makes `up` exit today.
- **Spec amendments** listed under the decisions.

**Carried from plan (2), unchanged:**
- the frontend's question cards;
- push for `activity → blocked`, and for a question asked outside a turn;
- `elicitation/complete`;
- `fs/*` and `terminal/*`;
- a question that blocks a start;
- delete of `pending` and `answer_queue` rows (§4.10);
- the live elicitation gate with real adapters;
- (2)'s spec amendments, still to be applied, with B2a/B2b's drift items, in one docs PR;
- `cancel_questions` on a turn cancel also cancelling questions asked outside the turn;
- the frontend rendering `PendingItem{state: cancelled, delivered: true}`;
- the wall-clock budgets in older tests.

**Carried from B2b, unchanged:**
- legacy model and mode switching;
- the New-session pickers before a session exists;
- the rest of the catalogue;
- `model` / `mode` in the list and detail items;
- an adapter that answers switches without a catalogue;
- a timed-out live switch that still lands;
- an adapter that exits with a switch out answers it `config_failed`;
- the unbounded drain;
- the flaky CLI test `sigint_to_ups_process_group_still_shuts_down_cleanly`;
- B2a's `images` / `projects` capabilities, the fixed `CANCEL_GRACE`, and the `hello.capabilities` doc.

Then, in order:
- **(3b) Operator auth**
- **(3c) Passkeys**
- **(4) Frontend shell**, including the question cards
- **(5) Hats**
- **(6) Gateway**
- **(7) Distribution**

---

_Generated with Claude AI — please review before distribution._
