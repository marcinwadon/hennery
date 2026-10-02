# MCP gateway (plan 8a): connections and credentials at rest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ] `) syntax for tracking.

**Goal:** The operator can keep MCP connections in the collector, mount them on hosts and give a static token to one, with every secret sealed at rest (gateway spec §1, §2, §4.6, §6, §9):
- a new crate, `hennery-gateway`, with its own migration component (`gateway`) in `hennery.db`;
- the master key: `<data>/master.key` (made at the first start, private, checked on every load), `HENNERY_MASTER_KEY` or a systemd credential; a key missing while credentials are stored, or one that does not open them, stops the start;
- credentials sealed with XChaCha20-Poly1305, bound to their row, their field and their key version;
- `GET/POST /api/mcp/connections`, `PATCH/DELETE /api/mcp/connections/{id}`, `PUT …/{id}/mounts` and `PUT …/{id}/credential`, behind the operator's session, and step-up where a token's reach changes;
- another origin or another kind deletes the credential in the change's own transaction (§4.6);
- `purge_hat`, for the purge lane's hook;
- the API documented as a contract: every wire type, field, route answer and error code, in the generated TypeScript too (Task 5).

No proxy, session tokens, OAuth, egress, renderers or standalone mode: plans 8b–8h.

**Architecture:**
- **Gateway** (`hennery-gateway`, new; depends on `hennery-kernel` and `hennery-proto`, never on `hennery-sessions`):
  - `key.rs`: `MasterKey` (zeroizing, `Debug` shows only its version), `KeySource`, `load_or_create`.
  - `crypto.rs`: `seal` and `open`, the AAD and the blob layout.
  - `model.rs`: `ConnectionRecord`, `NewConnection`, `ConnectionPatch`, `Change`, `CredentialChange`, `CredKind`, `StaticCredential`, `url_for_logs` and the checks on each part.
  - `schema.rs`: the `gateway` component's migration: `gw_connections`, `gw_credentials`, `gw_mounts`.
  - `store.rs`: `GatewayStore`, its own connection to `hennery.db`; every SQL statement of the crate, each naming the owner. It joins the owner audit.
  - `api.rs`: `GatewayState` and `router`.
  - `lib.rs`: `open`, the gateway on `hennery.db` with its key, and `KeyUnavailable`.
- **Wire** (`hennery-proto`): `McpCredKind`, `McpConnectionStatus`, `McpConnectionItem`, `CreateMcpConnectionRequest`, `UpdateMcpConnectionRequest`, `McpMountsRequest`, `McpCredentialRequest` (its `Debug` redacts the token), each with its route, answers and error codes in its doc. `render_ts` writes each type's own doc above it (it wrote only the fields' before), so the docs of every wire type reach the TypeScript. The schema and TypeScript files are regenerated.
- **Binary** (`hennery`): `run_collector` calls `hennery_gateway::open` after the admin socket's bind and before serving, then merges the gateway's router.
- **Host** (`hennery-host`): `HENNERY_MASTER_KEY` joins `HOST_SECRET_VARS`, so `up`'s host child and every agent are spawned without it.
- **Tests:** the gateway's own (`tests/{key,crypto,store,owner,boundary,api,api_log,open}.rs`, and the unit tests of `crypto.rs` and `store.rs`), the owner audit (`hennery-testkit/tests/owner_filter.rs`), the CLI (`crates/hennery/tests/cli.rs`), and the generated files' (`crates/hennery-proto/tests/codegen.rs`).

**Tech Stack:** Rust (edition 2024, MSRV 1.88), rusqlite 0.40, tokio, axum 0.8. New crates, pinned in `[workspace.dependencies]`:
- `chacha20poly1305 = "=0.11.0"`, default features off, `alloc` and `zeroize` on: XChaCha20-Poly1305, which gateway spec §6 names; no random number generator of its own (the nonces come from `hennery_kernel::secret::random_bytes`, the OS generator), and the cipher's key state wiped when dropped. It brings `aead 0.6.1`, `cipher 0.5.2`, `poly1305 0.9.1`, `universal-hash 0.6.1`, `inout 0.2.2`, `hybrid-array 0.4.15`, `crypto-common 0.2.2`, `block-buffer 0.12.1`, `ctutils 0.4.2`, `cmov 0.5.4`; `chacha20 0.10.2` is in the lockfile already.
- `zeroize = "=1.9.0"`, in `[workspace.dependencies]` already (plan 10a's VAPID key): the master key and the opened secrets in memory.
- Dev-dependencies of the gateway, all in the workspace already: `ed25519-dalek` (hosts' keys in tests), `rusqlite` with `hooks` (the store's unit test), `tempfile`, `tokio`, `toml` (the boundary test), `tower` (driving the router), `tracing-subscriber` (the log capture).

`Cargo.lock` changes in Step 1 of each task, by one `cargo build --workspace` without `--locked`; every other command runs `--locked`.

**Spec:** [`docs/specs/2026-09-26-mcp-gateway-design.md`](../specs/2026-09-26-mcp-gateway-design.md), the [kernel spec](../specs/2026-09-26-kernel-design.md) and the umbrella [`docs/specs/2026-09-25-hennery-architecture-design.md`](../specs/2026-09-25-hennery-architecture-design.md):
- gateway §1: a connection belongs to one hat; ticking a host is a mount.
- gateway §2: `gw_connections(id, owner_id, slug UNIQUE, label, url, hat_id, cred_kind, tool_allowlist JSON, internal_network, status, status_note, account_label, status_at, created_at, updated_at)`, `gw_credentials(connection_id PK, key_version, ciphertext BLOB, expires_at, updated_at)`, `gw_mounts(connection_id, host_id)`; "List endpoints never read those tables"; slugs `^[a-z0-9][a-z0-9-]{0,47}$`; a static token "sent as `Authorization: Bearer <token>` by default, or under a configured header name with an optional value prefix" (maintainer decision 6d); hat purge deletes the hat's connections with their credentials and mounts.
- gateway §4.6: "Changing a connection's URL origin or its `cred_kind` deletes its credential and OAuth client before the change is saved. … An omitted field in an update keeps its stored value; an explicit empty value clears it."
- gateway §6: "AEAD: XChaCha20-Poly1305, random 24-byte nonce per write, AAD = `connection_id ‖ field ‖ key_version`, stored as `key_version ‖ nonce ‖ ciphertext`. Master key: 32 random bytes generated on first run at `<data>/master.key` (created exclusively, mode 0600, permissions checked on load), or supplied through an environment variable or a systemd credential. Held in zeroizing memory. `key_version` exists from day one."
- gateway §9: the routes; "Create (step-up)"; "Update (step-up when the URL, credential kind or `internal_network` changes)"; "Set a static token (write-only, 204; step-up)".
- gateway §10, umbrella §9: "the `hennery-gateway` crate does not depend on `hennery-sessions`".
- gateway §11: "Encryption: AAD binding (swapping ciphertext between rows fails), key rotation, missing key → clear error"; "Credential edits: origin change and kind change clear credentials"; "Token hygiene: no token … appears in logs".
- kernel §3.4: step-up for "creating or editing gateway connection URLs, credentials, pre-registered clients or the 'internal network' flag".
- kernel §10: `master.key` stays a 0600 file in v1 (maintainer decision 1).

It builds on the merged hats plans [5a](2026-10-06-hats.md) (composite foreign keys, decision 3) to [5d](2026-10-09-reassign-hat.md), and on the lane note's decisions L1, L6 and L8 (plan 8's split, kept with the gateway lane). Every anchor was taken from `main` at `ef75d4f`. Where the code and a spec disagree, the code wins, and the plan says so.

**Status:** executed 2026-10-02 (see "Execution status"); amended after the security review and its two re-confirmations, whose answers are under "The security review's answers".

**How the code blocks were made and checked:**
- Every block below was generated from a scratch branch on `ef75d4f`, two commits per task: the task's tests alone, then the whole task.
- The plan was replayed from its own text onto a fresh worktree of `ef75d4f`: after each Step 1 (and its `cargo build`) the tree matched the tests-only commit, and after each task the task commit, byte for byte, `Cargo.lock` and the generated files included.
- After every task the five checks passed. The workspace has 1075 tests at the end, up from 1019 on `ef75d4f`.
- Every guard and side-effect line was revert-probed (each task's Step 5): 94 probes, each caught by the test named. Task 4's were run again after the rebase onto `0955dce`; the later rebase onto `ef75d4f` (plans 8b and 10b-i) touched no file of 8a's but `Cargo.lock` and the generated files, and merged without a conflict.

## Execution status (2026-10-02)

**Executed** on branch `feat/gateway-8a-store`, rebased onto `main` at `d2d2894` (plan 9a, 10b-iii, the SSE fix, the adapter process-group fix, the first-run Node fix and CI's concurrency merged meanwhile), then onto `522e802` (10b-ii push delivery, #71, and 8b-ii, #75): the conflicts in `Cargo.lock` (main's lock, 8a's crates added by cargo), `codegen.rs` and `rest.rs` (both sides' types kept), `main.rs` (push's `Egress` and delivery as main has them, the gateway's key, store and router beside them) and the generated files (regenerated) were resolved keeping both sides. The plan's anchors are `ef75d4f`'s. Each task was applied from the plan's own text (`replay.py`, Step 1 then the rest) by one implementer, then reviewed (opus). Every task tree matched the scratch reference byte for byte, outside `docs/plans`. The rebase merged without a conflict; the generated files regenerated unchanged.

| Area | As built | Why |
|---|---|---|
| Tasks 1–4 | As written. Every review approved. | Their minor findings are under "After this plan" (follow-ups). |
| Task 5 review: changes required, one fix round | The docs of `label` say "1 to 64 bytes of UTF-8 once trimmed (stored trimmed), with no control or invisible format character"; `McpConnectionItem.url` is whole in every answer of the API, not only the list; the mounts limits count the ids as sent, before the connection is looked up. A scoped re-review confirmed it. These lines on the branch supersede the same lines in Task 5's blocks. | The label check (`hosts::is_displayable_text`) counts UTF-8 bytes, so a frontend using `maxLength=64` would accept labels the server refuses. |
| Whole-branch review: ready after fixes | `internal_network`'s doc says plain `http` goes only to an internal address, under 8b-ii (merged first; at the review it said "from plan 8b-ii on"). Decision 13 says that only an id over 64 bytes is not echoed back (an unknown or revoked host's id is named in the error). The file table names `hennery_gateway::open` and only `chacha20poly1305` as new. | 8b's `check_url` sends plain `http` only to loopback under every allowance until 8b-ii (L12); the wire doc promised more. The rest were the plan's prose, not the code. |
| Spec write-back | The "Spec amendments" below are in the gateway spec (§2, §4.6, §5.7, §5.8, §6, §9 as built, §11), the kernel spec §3.4 and ACP core §1. | |
| Lane rulings (2026-10-02) | Q5 (the origin and owner in the AAD) is not a maintainer question: declined by the security review, confirmed by the lane parent; decision 7 stands. The `http`-to-loopback mismatch with 8b's egress stays as reviewed: 8a refuses it at save time, which is stricter; 8b-ii and 8d settle it. | The lane parent's rulings. |

Checks:
- After every task the five checks passed: 1040, 1058, 1069, 1073 and 1075 tests, from 1019 on `ef75d4f`.
- After the rebase onto `522e802` the five checks passed again, with 1153 tests in the workspace (plan 8a's 56 on top of `main`'s 1097).
- The 94 revert-probes were run when the plan was built (`probes.log` in the ledger); execution applied the same code.
- The run was macOS only, so ubuntu CI is the Linux check.

## Scope

Plan 8 is split into 8a–8h (the gateway lane's note). 8a is wave 1 with 8b (egress) and 8c (the host side of delivery), and depends on neither. That is **5 tasks**:
1. the master key and credentials at rest;
2. connections, mounts and the static credential in the store;
3. the connections API, behind step-up;
4. the collector opens the gateway and serves its routes;
5. the API documented as a contract, into the TypeScript.

**Out:**
- the proxy, session tokens, `ClientIdentity`/`MountPolicy` (8d); the sessions wiring and `SessionMcp` (8e); OAuth, the probe, status and `Notifier` (8f); standalone clients, renderers, `rotate-key` and `up`'s multi-hat warning (8g); the composed `CODEX_HOME` (8h); the egress policy (8b);
- wiring `purge_hat` into `LifecycleHooks::on_hat_purged`: the purge lane adds the hook (L6); whoever merges second wires it;
- the frontend's MCP view (plan 4).

## Decisions this plan makes where the spec is silent

The security reviews of 2026-10-02 answered these on the maintainer's behalf; their answers, and the two questions left open for the maintainer (Q1, Q2), are under "The security review's answers". Amended decisions say which finding amended them.

1. **A slug is unique per owner** (`UNIQUE (owner_id, slug)`), not across the installation (§2 says "unique per installation").
   - v1 has one owner per installation, so the two read the same today.
   - Unique across owners, one owner's create would learn another's slugs from `slug_taken`, which the owner-everywhere design (umbrella §7.4) rules out. The proxy (8d) finds a connection by owner and slug: the token names the owner.
2. **The tables.** Every `gw_*` table has `owner_id` (L6). A connection's hat is the owner's, `FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id)`; a mount's host likewise, `hosts(id, owner_id)`; a credential's and a mount's connection likewise, `gw_connections(id, owner_id)`.
   - Nothing cascades. The store deletes a connection's credential and mounts itself, and a hat with connections cannot be deleted until the gateway's purge has run (kernel §5.5, L6).
   - The `CHECK`s list every value §2 has, the OAuth kinds and every status included: these tables will have children, and the migration runner cannot rebuild a table with children to widen a `CHECK` later (kernel `schema.rs`, migration 6's note).
   - The static token's header and prefix (decision 6d) are columns of the connection, `static_header` (default `Authorization`) and `static_prefix` (default `Bearer `): they are not secret, and the list shows them.
   - The hat is checked as the owner's by a query in the create's transaction (400 `invalid`), not left to the foreign key, whose violation would be a 500.
3. **`has_credential` without reading the secret.** The list's statement asks `EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1)`. A unit test in `store.rs` prepares the list's statements under SQLite's authorizer: they read `connection_id` and `owner_id` of `gw_credentials`, and nothing else. §2's "List endpoints never read those tables" is read as "never read their secret columns" (spec amendment).
4. **Step-up.**
   - `POST` (create), `DELETE`, and `PUT …/credential`, layered per method (`Handler::layer`), so `GET` stays free and a method added later gets none unless it is layered too.
   - `PATCH` when it names `url`, `cred_kind`, `internal_network`, `static_header` or `static_prefix`, checked in the handler on what the body names, before anything is read: a field named with its stored value still needs it. §9 names the first three; the header and the prefix decide how the same token is sent, so they join them.
   - `DELETE` is not marked in §9. It destroys a grant that may need a consent to get back, as revoking a host or a session does (kernel §3.4), so it needs step-up.
   - `PUT …/mounts` needs none, as §9 has it: a mount reaches only a host the owner paired, and pairing needs step-up.
   - Renaming (`label`) and the allowlist need none.
   - Step-up on `DELETE` (Q4) and no `DELETE …/credential` route (Q3) were confirmed by the gateway lane parent.
5. **The slug and the hat never change.** `UpdateMcpConnectionRequest` has neither, and refuses unknown fields (`deny_unknown_fields`, 422 `invalid_body`), so `{"hat_id": …}` is refused rather than ignored.
   - Moving a connection to another hat would carry its grant into that hat (umbrella §8.3: the same vendor in two hats is two connections). A new slug would rename the agents' server (`hennery-<slug>`).
   - Every request type of the gateway refuses unknown fields.
6. **The master key's sources.** (Amended: the reviews' R3, O1, O2.)
   - In order: `HENNERY_MASTER_KEY` (64 hexadecimal digits); the systemd credential `$CREDENTIALS_DIRECTORY/hennery-master-key` (32 raw bytes, or 64 hexadecimal digits with an optional newline, for `SetCredential=`); `<data>/master.key` (32 raw bytes).
   - Both of the first two at once is refused: which one sealed the stored credentials would be a guess. A supplied key with a `master.key` beside it warns that the file is not used.
   - A source the operator set that cannot be read is an error, never a reason to make a key file: a `HENNERY_MASTER_KEY` that is not text, and a credential path whose look-up fails other than "not found" (`EACCES`, `ENOTDIR`). `$CREDENTIALS_DIRECTORY` set without the credential warns before a key file is made (a mistyped `LoadCredential=` name).
   - `master.key` is opened without following a symlink and checked through its descriptor: a regular file, no group or other bits (`chmod 600`), one link (a hard link is a second name its mode does not show), exactly 32 bytes. Its owner is not compared with the collector's user: the collector cannot read a 0600 file of another user but as root, and the mode and link checks refuse one it could read. (The data directory's 0700 is only warned about when looser, in `private_data_dir`, so this does not rest on it: the re-confirmation's correction.) A credential file: a regular file, not a symlink, no bits for other users; systemd owns it (often root, with an ACL for the service's user), so its owner and group bits are not checked.
   - A new `master.key` is created exclusively (`O_EXCL`, `O_NOFOLLOW`), 0600 whatever the umask, written and synced, and its directory synced, so a crash cannot lose the entry while `hennery.db` keeps credentials sealed under it. A write cut short leaves a file of the wrong length, which the next start refuses rather than replaces.
   - Missing while the owner has a stored credential: an error naming the restore, the two other sources and the way to give the credentials up (decision 8); no new key.
   - The variable is read through `std::env::var_os`, not a clap `env =` argument (clap would show its value in `--help`). It joins `HOST_SECRET_VARS`: `up` passes its environment to its collector, and the host child and every agent are spawned without it. `docker run -e` shows it in `docker inspect`; the docs say so (8g).
   - Error messages never quote the key.
7. **The AEAD's encoding.** (Amended: the review's R1 and O5.)
   - AAD = `len ‖ connection_id ‖ len ‖ field ‖ len ‖ key_version`, each `len` 4 bytes big-endian, the version 4 bytes big-endian: no two (id, field) pairs give one AAD. A unit test opens a version-1 blob as version 2 with the same key and nonce and sees it refused: the version is bound by the tag, not only checked beside it.
   - The blob is `key_version` (4 bytes, big-endian) ‖ the 24-byte nonce ‖ the ciphertext with its 16-byte tag. The row's `key_version` column must equal the blob's prefix (else `Malformed`), and both must be the key's (else `KeyVersion`).
   - The field is named by table and kind, `gw_credentials.static_token`, and taken from the connection's kind: a row left under another kind does not open as another field (G-14, a second guard behind §4.6), and later tables (`gw_oauth_clients`, `gw_stdio_servers`) never share a field with this one.
   - The origin and the owner are not in the AAD (the review's O5; Q5: **declined by the security review, confirmed by the lane parent**, 2026-10-02). Binding them would cost little: §4.6 already deletes the credential on every origin change, and the owner never changes. (An earlier draft argued that binding the URL would force a re-seal on every edit; the re-confirmation pointed out that O5 asked for the origin, not the URL.) What it would buy: someone who can write `hennery.db` but not read the key (a copied volume or a restored backup, the key supplied by the environment or systemd) could not point a stored token at another origin. Whether that attacker is in the threat model, kernel §10 does not say. Not bound; reversible by a re-seal at start with the key, as `rotate-key` does (once 8a serves its routes, real tokens can be stored). 8d does not wait on it.
   - The version is `1` (`KEY_VERSION`) until `rotate-key` (8g) adds others.
8. **A key that does not open the stored credentials stops the start.** (Amended: the review's finding 1; Q1 below.)
   - At start the collector opens the owner's newest credential (`check_key`); failing, it does not start, and neither does it when the key is missing while credentials are stored. Starting would seal new rows under one key beside rows only another opens. Only the newest row is opened: a damaged older row shows when it is used, and does not stop the start.
   - Every such error says how out: restore the key, or stop the collector and give the credentials up (`sqlite3 <data>/hennery.db 'DELETE FROM gw_credentials'`), then set each one again. The status of those connections stays as it was until the probe (8f).
   - `hennery_gateway::open` wraps every key error in `KeyUnavailable`, so the binary can tell it from a store that does not open. That keeps the alternative below a small change in `run_collector`.
   - **Q1, open for the maintainer (default chosen, reversible).** What should a lost or wrong key take down?
     - *Refuse to start* (the default). Costs: every session, host and login is down until the key is restored or the credentials are given up, through a database command, since no route is served. Gains: nothing ever runs with a gateway it cannot open; the operator cannot miss it; no code path exists that serves the gateway half-working.
     - *Start with the gateway off*: its routes answer 503 `gateway_key_unavailable`, sessions run without gateway servers. Costs: a second state to build and test (a stand-in router, the 503s, and 8d/8e/8f each learning that the gateway can be absent); sessions start quietly without their integrations, which an operator may notice only later; recovery still needs the key or the database command, or a new route. Gains: a lost key costs only the integrations.
     - Switching is `run_collector` matching `KeyUnavailable` and merging a 503 router instead (`hennery_gateway::open` already separates the case); 8e would then treat the gateway as absent.
   - Not every key error names the way out yet (the re-confirmation's finding 3): a `master.key` of the wrong length, a systemd credential that does not read, and `KeySource::from_env`'s own errors, which `run_collector` gets before `open` and so without `KeyUnavailable`. Recorded for 8g, which owns `rotate-key` and the key's docs; if Q1 is answered the other way, `from_env`'s errors join `KeyUnavailable` first.
9. **The URL.** `http` or `https`, absolute, with a host, no user name or password, no fragment, at most 2048 bytes; stored as `url::Url` serialises it. `http` only when the connection is marked `internal_network`: a token sent in clear across the public internet is the case to refuse.
   - The operator's decision of 2026-10-02, through the gateway lane: a connection marked `internal_network` may use plain `http` to a private or LAN address, on create or by one `PATCH` naming both (step-up). Unmarked, `http` is refused at save time, loopback included. Clearing the mark while the URL is `http` is **refused (400 `invalid`) and changes nothing**, not taken with the credential cleared: the operator moves the URL to `https` in the same `PATCH`. Pinned by `http_stays_only_while_the_connection_is_internal` (Task 2) and `an_internal_connection_may_use_http_and_keeps_its_mark_while_it_does` (Task 3); no code changed. The lane's note gives the egress default for unmarked connections as `https` or `http` to loopback; 8a refuses unmarked `http` to loopback when saving, which 8b-ii settles.
   - A query is allowed; a secret belongs in the credential. The list answers the URL as stored, to its owner; nothing else shows more than its origin (decision 19).
   - **Q2, open for the maintainer (default chosen, reversible).** Some vendors put the secret in the URL (`/s/<secret>/mcp`). The default is to document it: the URL is the owner's data, listed behind auth and never logged whole. Sealing such URLs, or refusing them, is the maintainer's call before 8d.
   - Non-public addresses are not refused when saving. The egress policy (8b) refuses them at request time unless the connection is internal (L7).
   - The origin compared by §4.6 is `Url::origin()`: scheme, host and port, the default port filled in.
10. **OAuth kinds wait for 8f.** `oauth_dcr` and `oauth_client` are refused, on create and on update, with 400 `unsupported_cred_kind`; the store refuses them too, so 8f lifts one check. The schema takes them already (decision 2).
11. **Credential invalidation (§4.6).** In the update's transaction, before the row is saved: another origin or another kind deletes the credential and starts the status over (`not_connected`, no note, no account label, `status_at` now). Another path, header, prefix or internal flag keeps it; each of those needs step-up (decision 4). A refused update deletes nothing.
    - Absent keeps, explicitly empty clears: a `null` allowlist clears it (every tool); `[]` allows none; `""` clears the prefix. The label, URL, kind and header cannot be empty. A `null` for another field reads as absent (it changes nothing, so needs no step-up).
    - 8f must delete the OAuth client in the same place.
12. **A revoked host's mounts are kept, unused and unlisted.**
   - Revoking a host does not reach the gateway. Its mounts stay, and the list leaves them out (it joins `hosts` on `revoked_at IS NULL`). A `PUT …/mounts` naming a revoked host is refused, and the next full set drops it.
   - A revoked host never becomes live again: ids are primary keys, a revoke is never undone, and the key of a revoked host cannot pair again.
   - 8d and 8e must apply the same join wherever a mount decides delivery or access; 8d's `on_host_revoked` may delete them instead (the review's N3), and must before any plan deletes host rows.
13. **Limits.** (Amended: the review's O3, N6.) At most 256 connections per owner (409 `too_many_connections`). A label is 1 to 64 bytes of UTF-8 once trimmed, with no control or invisible format character (amended at execution: the check counts bytes). An allowlist names at most 1024 tools, each 1 to 128 visible ASCII characters; duplicates are dropped, the order kept. A mounts request names at most 1024 hosts, each id at most 64 bytes, checked before the transaction; an id over 64 bytes is not echoed back (amended at execution: an unknown or revoked host's id is named in the error). A static token is 1 to 8192 visible ASCII characters, no space, nothing a header breaks on. The header is an HTTP token of at most 64 bytes, not one the proxy sets or filters (`host`, `content-length`, `content-type`, `content-encoding`, `transfer-encoding`, `connection`, `keep-alive`, `upgrade`, `te`, `trailer`, `cookie`, `accept`, `accept-encoding`, `mcp-session-id`, `mcp-protocol-version`, `last-event-id`, `expect`, `forwarded`, `via`, `max-forwards`, `proxy-*`, `sec-*`). The prefix is at most 32 visible ASCII characters or spaces. A body is at most 256 KiB (413 `body_too_large`).
14. **The credential route, and its read.** (Amended: the review's R2, O8.) `PUT …/credential {token}` answers 204. A connection that is not `static` answers 409 `wrong_cred_kind`; an invalid token 400 `invalid`, with a message that does not quote it. No route reads a credential back, and none clears one: changing the kind or deleting the connection does (Q3, confirmed below).
    - `GatewayStore::static_credential` answers `StaticCredential {token, url, static_header, static_prefix, internal_network}`, from one statement: an edit that moves the URL cannot land between reading the token and reading where it goes. Its `Debug` redacts the token. The opened bytes are moved out of their zeroizing buffer, not copied, and wiped on the error path too.
15. **The answers.** 404 `not_found` for an unknown connection or another owner's, on every route; 409 `slug_taken`; 400 `invalid` with the reason; 422 `invalid_body` for an unknown field or a wrong type, from `ApiJson`, which never quotes the body back. A connection's answers are `McpConnectionItem`, stamps in RFC 3339; `DELETE` answers 204. An error's body is the shared `ApiError`, with exactly `code` and `message` (the fleet parent's ruling of 2026-10-02, pinned by `an_error_is_the_shared_api_error_and_nothing_more`).
16. **`has_ciphertext` and `check_key` read the owner's rows only**, like every query (kernel §1). With one owner in v1 that is every row; a second owner's credentials would not stop a new key being made (recorded for 8g's `rotate-key`).
17. **Status.** 8a sets only `not_connected` (at creation, and when §4.6 starts it over). The probe (8f) sets the others.
18. **The router.** The binary merges the gateway's router beside the sessions module's: `hennery-sessions` does not depend on the gateway until 8e's `SessionMcp`. The kernel's CSP layer, applied inside the sessions router, does not cover the gateway's routes, which answer only JSON.
19. **An upstream URL is shown only as its origin** (lane note L11, from the fleet parent, whatever Q2's answer). No log line, error body, trace or `Debug` of a connection type shows more of an upstream URL than `scheme://host[:port]` (`model::url_for_logs`, `rest::url_origin`), with the connection's id. `ConnectionRecord`, `NewConnection`, `ConnectionPatch`, `StaticCredential` and the three wire types have their `Debug` by hand. The list API still answers the URL as stored: that is the owner's data, behind auth. A test puts a canary in the URL's path and query, runs create, patch, list and refused requests at `TRACE`, and finds the canary in no log line and no error body. `url_origin` parses with `url::Url`, as the store does, so raw input in a request's `Debug` fails closed (`<not a url>`, or `null` for a scheme without an origin): the re-confirmation's finding 1, with the inputs it named in the canary test, and a 422 body whose canary is a string in a boolean field, where serde's own message would quote it (round 2's note 2).
20. **No cache keeps an answer** (the review's O6): every gateway route answers `Cache-Control: no-store`, as `projects.rs` does for private answers. The layer is outside `operator_only`, so the session's and the browser rules' refusals carry it too (the re-confirmation's finding 4).
21. **The API is a contract** (from the gateway lane parent, 2026-10-02, after the reviews: the frontend lane builds the MCP screens on it). Every gateway wire type, each field and each variant has a doc; each request type's doc names its route, whether it needs step-up, its answer and every error code it adds; `McpConnectionItem`'s names the codes every route may answer (from `operator_only`, `ApiJson` and the handlers) and `DELETE`'s. Tests read the schema for a doc on every field and variant, and the TypeScript for each type's doc and each code.
    - ts-rs's `decl`, which `render_ts` used, leaves out a type's own doc; ts-rs's own export writes `TS::docs()` above it, and so does `render_ts` now. That adds every existing type's doc to `protocol.ts` too, a generated-file change only: other lanes regenerate it when they rebase, as they must anyway.
    - Names stay as reviewed: no wire type, field or code was renamed.

## Global Constraints

- Rust edition 2024, `rust-version = "1.88"`; licence `AGPL-3.0-only`; crates are `publish = false`; crate names prefixed `hennery-`.
- New crates pinned exactly (`=x.y.z`) in `[workspace.dependencies]`, each with its reason.
- After every task the five checks pass: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p hennery --locked -- -D warnings`; `cargo test --workspace --locked`; `cargo run -p hennery-proto --bin gen -- --check`.
- `hennery-gateway` never depends on `hennery-sessions`, in any table (umbrella §9).
- Every query names the owner; the gateway's `store.rs` joins the owner audit.
- Every route is the operator's (`operator_only`), and takes its body as `ApiJson`.
- No token in a log line, an answer, or `hennery.db` unsealed.
- **No Linux-only code**, and no test reading another process's state without polling for a positive signal (fleet rule).
- Commits: Conventional Commits, gmail identity, unsigned. Push after every task; never push `main`.

## Review Focus

1. **A credential sent to a host the operator did not mean** (an edited URL, kind or internal flag).
   - Expected: another origin or kind deletes the credential in the same transaction; a refused edit deletes nothing; editing needs step-up.
   - Tests: Task 2 `another_origin_or_kind_deletes_the_credential`; Task 3 `where_a_token_can_go_changes_only_with_a_fresh_step_up`, `a_static_credential_is_write_only`.
2. **A collector started without its key, or with another one.**
   - Expected: refused, no new key, a message saying what to restore.
   - Tests: Task 1 `a_missing_key_while_credentials_exist_is_an_error_and_creates_nothing`; Task 4 `the_collector_keeps_its_master_key_and_will_not_start_without_it`.
3. **A token in a log, an answer or the database file.**
   - Expected: none, at `TRACE`, for a taken request and for each kind of refusal.
   - Tests: Task 3 `a_static_token_is_never_logged_answered_or_stored_in_clear`.
4. **Another owner's connection named by id, slug, hat or host.**
   - Expected: 404 or `invalid`, nothing of theirs read or changed.
   - Tests: Task 2 `another_owners_connections_are_invisible_to_the_store` and the owner audit.
5. **A master key file others can read, or a symlink planted in its place.**
   - Expected: refused, the file left as it is.
   - Tests: Task 1 `a_key_file_others_can_read_is_refused_and_left_alone`, `a_symlinked_or_odd_key_file_is_refused`; Task 4's mode check under `umask 022`.

## File structure

| Path | Responsibility | Task |
|---|---|---|
| `Cargo.toml`, `Cargo.lock` | `chacha20poly1305` (new; `zeroize` was in the workspace already), `hennery-gateway` | 1–4 |
| `crates/hennery-gateway/Cargo.toml`, `src/lib.rs` | The crate | 1–3 |
| `crates/hennery-gateway/src/key.rs` | The master key | 1 |
| `crates/hennery-gateway/src/crypto.rs` | Sealing and opening | 1 |
| `crates/hennery-gateway/src/model.rs` | A connection and its checks | 2 |
| `crates/hennery-gateway/src/schema.rs` | The `gateway` component's migration | 2 |
| `crates/hennery-gateway/src/store.rs` | `GatewayStore`: every statement | 2 |
| `crates/hennery-gateway/src/api.rs` | The routes | 3 |
| `crates/hennery-proto/src/rest.rs`, `codegen.rs`, generated files | The wire types, and their docs in the TypeScript | 3, 5 |
| `crates/hennery/Cargo.toml`, `src/main.rs` | `hennery_gateway::open` in `run_collector`, the merged router | 4 |
| `crates/hennery-host/src/adapter.rs` | `HENNERY_MASTER_KEY` in `HOST_SECRET_VARS` | 4 |
| Tests: `crates/hennery-gateway/tests/{key,crypto,store,owner,boundary,api,api_log}.rs`; `crates/hennery-testkit/{Cargo.toml,tests/owner_filter.rs}`; `crates/hennery/tests/cli.rs`; `crates/hennery-proto/tests/codegen.rs` | | 1–5 |

**Reading the steps:** each code block is preceded by exactly one of these instructions, and it means exactly this:
- "Create `path`:" makes a new file with the block.
- "Replace the whole of `path` with:" overwrites the file with the block.
- "Append to `path`:" adds a blank line, then the block, at the end of the file.
- "In `path`, replace:" is followed by a block that occurs **exactly once** in the file at that point, as whole lines (earlier blocks of the same task already applied, in order), then "with:" and its replacement.

Other "Run:" lines only check or regenerate: `cargo build --workspace` rewrites `Cargo.lock`, and `cargo run -p hennery-proto --bin gen` the generated files. They change no other file. Run everything as `nix develop -c <cmd>` from the worktree's root.

---

### Task 1: The master key and credentials at rest

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`
- Create: `crates/hennery-gateway/Cargo.toml`, `src/lib.rs`, `src/key.rs`, `src/crypto.rs`
- Test: `crates/hennery-gateway/tests/key.rs`, `tests/crypto.rs`

**Interfaces:**
- Produces:
  - `hennery_gateway::key::{MasterKey, KeySource, KeyOrigin, load_or_create, KEY_FILE, KEY_ENV, CREDENTIAL_NAME, KEY_VERSION}`;
  - `MasterKey::from_bytes([u8; 32]) -> MasterKey`, `MasterKey::version(&self) -> u32`;
  - `load_or_create(&KeySource, ciphertext_exists: bool) -> anyhow::Result<(MasterKey, KeyOrigin)>`;
  - `KeySource { data_dir: PathBuf, env: Option<Zeroizing<String>>, credentials_dir: Option<PathBuf> }`, `KeySource::from_env(&Path)`;
  - `hennery_gateway::crypto::{seal, open, CryptoError, STATIC_TOKEN, NONCE_LEN}`: `seal(&MasterKey, connection_id, field, plaintext) -> Vec<u8>`, `open(&MasterKey, connection_id, field, key_version: u32, blob) -> Result<Zeroizing<Vec<u8>>, CryptoError>`.
- Consumes: `hennery_kernel::secret::random_bytes`.

- [ ] **Step 1: Write the failing tests**

In `Cargo.toml`, replace:

```toml
clap = { version = "4", features = ["derive", "env"] }
ed25519-dalek = "=2.2.0"
```

with:

```toml
clap = { version = "4", features = ["derive", "env"] }
# The MCP gateway's credentials at rest (plan 8a): XChaCha20-Poly1305.
# Default features off: no random number generator of its own (nonces come
# from the kernel's), and its key state zeroized when dropped.
chacha20poly1305 = { version = "=0.11.0", default-features = false, features = ["alloc", "zeroize"] }
ed25519-dalek = "=2.2.0"
```

In `Cargo.toml`, replace:

```toml
hennery-host = { path = "crates/hennery-host" }
```

with:

```toml
hennery-host = { path = "crates/hennery-host" }
hennery-gateway = { path = "crates/hennery-gateway" }
```

Create `crates/hennery-gateway/Cargo.toml`:

```toml
[package]
name = "hennery-gateway"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish.workspace = true

# Never `hennery-sessions`, in any table (umbrella §9, gateway spec §10):
# `tests/boundary.rs` reads the manifests and fails if it appears.
[dependencies]
anyhow.workspace = true
chacha20poly1305.workspace = true
hennery-kernel.workspace = true
hex.workspace = true
libc = "0.2"
thiserror.workspace = true
tracing.workspace = true
zeroize.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

Create `crates/hennery-gateway/src/lib.rs`:

```rust
//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).
```

Create `crates/hennery-gateway/tests/key.rs`:

```rust
//! The master key (gateway spec §6, kernel spec §10): 32 random bytes at
//! `<data>/master.key`, created once, private, checked on every load, or
//! supplied by the environment or a systemd credential. A key that is
//! missing while credentials exist is an error, never a new key.

use hennery_gateway::crypto::{STATIC_TOKEN, open, seal};
use hennery_gateway::key::{CREDENTIAL_NAME, KEY_FILE, KeyOrigin, KeySource, MasterKey, load_or_create};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn bytes() -> [u8; 32] {
    std::array::from_fn(|i| i as u8)
}

fn source(dir: &Path) -> KeySource {
    KeySource {
        data_dir: dir.to_path_buf(),
        env: None,
        credentials_dir: None,
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Whether `a` opens what `b` sealed: the two hold the same key.
fn same_key(a: &MasterKey, b: &MasterKey) -> bool {
    let blob = seal(b, "conn-1", STATIC_TOKEN, b"probe");
    open(a, "conn-1", STATIC_TOKEN, b.version(), &blob).is_ok()
}

fn error(source: &KeySource, ciphertext_exists: bool) -> String {
    format!("{:#}", load_or_create(source, ciphertext_exists).unwrap_err())
}

fn write_private(path: &Path, content: &[u8]) {
    std::fs::write(path, content).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn a_new_key_is_created_once_private_and_loaded_again() {
    let dir = tempfile::tempdir().unwrap();
    let (created, origin) = load_or_create(&source(dir.path()), false).unwrap();
    assert_eq!(origin, KeyOrigin::Created);
    let file = dir.path().join(KEY_FILE);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(std::fs::read(&file).unwrap().len(), 32);
    let before = std::fs::read(&file).unwrap();
    let (loaded, origin) = load_or_create(&source(dir.path()), true).unwrap();
    assert_eq!(origin, KeyOrigin::File);
    assert!(same_key(&loaded, &created));
    assert_eq!(std::fs::read(&file).unwrap(), before, "the key was replaced");
    // Two installs never share a key.
    let other = tempfile::tempdir().unwrap();
    let (theirs, _) = load_or_create(&source(other.path()), false).unwrap();
    assert!(!same_key(&theirs, &created));
}

#[test]
fn a_missing_key_while_credentials_exist_is_an_error_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let err = error(&source(dir.path()), true);
    assert!(err.contains("master.key") && err.contains("missing"), "{err}");
    assert!(!dir.path().join(KEY_FILE).exists());
}

#[test]
fn a_key_file_others_can_read_is_refused_and_left_alone() {
    for loose in [0o644, 0o640, 0o604, 0o620] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(KEY_FILE);
        std::fs::write(&file, bytes()).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(loose)).unwrap();
        let err = error(&source(dir.path()), false);
        assert!(err.contains("chmod 600"), "{loose:o}: {err}");
        assert_eq!(mode(&file), loose);
        assert_eq!(std::fs::read(&file).unwrap(), bytes());
    }
}

#[test]
fn a_symlinked_or_odd_key_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("elsewhere");
    write_private(&target, &bytes());
    std::os::unix::fs::symlink(&target, dir.path().join(KEY_FILE)).unwrap();
    let err = error(&source(dir.path()), false);
    assert!(err.contains("symlink"), "{err}");

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(KEY_FILE)).unwrap();
    let err = error(&source(dir.path()), false);
    assert!(err.contains("not a regular file"), "{err}");
}

#[test]
fn a_key_file_of_the_wrong_length_is_refused_and_never_replaced() {
    for content in [&bytes()[..31], &[0u8; 33][..], &[][..]] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(KEY_FILE);
        write_private(&file, content);
        let err = error(&source(dir.path()), false);
        assert!(err.contains("32 bytes"), "{err}");
        assert_eq!(std::fs::read(&file).unwrap(), content);
    }
}

#[test]
fn a_key_in_the_environment_is_used_and_no_file_is_made() {
    let dir = tempfile::tempdir().unwrap();
    let mut from_env = source(dir.path());
    from_env.env = Some(HEX.to_uppercase().into());
    let (key, origin) = load_or_create(&from_env, true).unwrap();
    assert_eq!(origin, KeyOrigin::Environment);
    assert!(same_key(&key, &MasterKey::from_bytes(bytes())));
    assert!(!dir.path().join(KEY_FILE).exists());
    // A malformed value is refused without being quoted back.
    for bad in [
        &HEX[1..],
        "zz02030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
        "",
    ] {
        from_env.env = Some(bad.to_string().into());
        let err = error(&from_env, false);
        assert!(err.contains("64 hexadecimal digits"), "{err}");
        assert!(bad.is_empty() || !err.contains(bad), "{err}");
    }
}

fn credentials(dir: &Path, content: &[u8], perm: u32) -> PathBuf {
    let creds = dir.join("credentials");
    std::fs::create_dir(&creds).unwrap();
    let file = creds.join(CREDENTIAL_NAME);
    std::fs::write(&file, content).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(perm)).unwrap();
    creds
}

#[test]
fn a_systemd_credential_is_used_raw_or_in_hex() {
    for (content, perm) in [
        (bytes().to_vec(), 0o400),
        (format!("{HEX}\n").into_bytes(), 0o440),
        (HEX.as_bytes().to_vec(), 0o600),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut from_creds = source(dir.path());
        from_creds.credentials_dir = Some(credentials(dir.path(), &content, perm));
        let (key, origin) = load_or_create(&from_creds, true).unwrap();
        assert_eq!(origin, KeyOrigin::Credential);
        assert!(same_key(&key, &MasterKey::from_bytes(bytes())));
        assert!(!dir.path().join(KEY_FILE).exists());
    }
}

#[test]
fn a_bad_systemd_credential_is_refused() {
    // Readable by everyone.
    let dir = tempfile::tempdir().unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(credentials(dir.path(), &bytes(), 0o404));
    assert!(error(&from_creds, false).contains("other users"));
    // The wrong length.
    let dir = tempfile::tempdir().unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(credentials(dir.path(), b"short", 0o400));
    assert!(error(&from_creds, false).contains("32 bytes"));
    // A symlink.
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("credentials");
    std::fs::create_dir(&creds).unwrap();
    let target = dir.path().join("elsewhere");
    write_private(&target, &bytes());
    std::os::unix::fs::symlink(&target, creds.join(CREDENTIAL_NAME)).unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(creds);
    assert!(error(&from_creds, false).contains("symlink"));
}

#[test]
fn a_credentials_directory_without_the_key_falls_back_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let creds = dir.path().join("credentials");
    std::fs::create_dir(&creds).unwrap();
    let mut from_creds = source(dir.path());
    from_creds.credentials_dir = Some(creds);
    let (_, origin) = load_or_create(&from_creds, false).unwrap();
    assert_eq!(origin, KeyOrigin::Created);
    assert!(dir.path().join(KEY_FILE).exists());
}

#[test]
fn two_supplied_keys_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut both = source(dir.path());
    both.env = Some(HEX.to_string().into());
    both.credentials_dir = Some(credentials(dir.path(), &bytes(), 0o400));
    assert!(error(&both, false).contains("both"));
}

#[test]
fn the_key_never_shows_in_debug_output() {
    let key = MasterKey::from_bytes(bytes());
    let shown = format!("{key:?}");
    assert!(!shown.contains("0, 1, 2") && !shown.contains("000102"), "{shown}");
    let mut from_env = source(Path::new("/nonexistent"));
    from_env.env = Some(HEX.to_string().into());
    let shown = format!("{from_env:?}");
    assert!(!shown.contains(HEX), "{shown}");
}

/// The review's R3: a source the operator chose that cannot be read is an
/// error, never a reason to make a key file in its place.
#[test]
fn a_key_source_that_cannot_be_read_is_refused_not_skipped() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    // `HENNERY_MASTER_KEY` set, but not text.
    let err = KeySource::from_vars(dir.path(), Some(std::ffi::OsString::from_vec(vec![0xff, 0xfe])), None)
        .expect_err("a non-UTF-8 key was taken as unset");
    assert!(format!("{err:#}").contains("64 hexadecimal digits"), "{err:#}");
    // `$CREDENTIALS_DIRECTORY` that cannot be looked into (a file, not a
    // directory: ENOTDIR).
    let not_a_dir = dir.path().join("not-a-dir");
    std::fs::write(&not_a_dir, b"x").unwrap();
    let source = KeySource::from_vars(dir.path(), None, Some(not_a_dir.into_os_string())).unwrap();
    let err = error(&source, false);
    assert!(err.contains("look for the credential"), "{err}");
    assert!(!dir.path().join(KEY_FILE).exists(), "a key file was made instead");
    // Unset, both: no source, and a key file is made.
    let source = KeySource::from_vars(dir.path(), None, None).unwrap();
    assert_eq!(load_or_create(&source, false).unwrap().1, KeyOrigin::Created);
}

#[test]
fn a_hard_linked_key_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(KEY_FILE);
    write_private(&file, &bytes());
    std::fs::hard_link(&file, dir.path().join("second-name")).unwrap();
    let err = error(&source(dir.path()), false);
    assert!(err.contains("hard links"), "{err}");
}

/// Plan 8a decision 8: the error says how to give the credentials up.
#[test]
fn a_missing_key_says_how_to_give_the_credentials_up() {
    let dir = tempfile::tempdir().unwrap();
    let err = error(&source(dir.path()), true);
    assert!(err.contains("DELETE FROM gw_credentials"), "{err}");
}
```

Create `crates/hennery-gateway/tests/crypto.rs`:

```rust
//! Credentials at rest (gateway spec §6): XChaCha20-Poly1305 with a random
//! nonce per write, AAD = connection id ‖ field ‖ key version, stored as
//! key version ‖ nonce ‖ ciphertext. A blob opens only for the row and the
//! field it was sealed for, under the key and the version it names.

use hennery_gateway::crypto::{CryptoError, NONCE_LEN, STATIC_TOKEN, open, seal};
use hennery_gateway::key::{KEY_VERSION, MasterKey};

fn key(seed: u8) -> MasterKey {
    MasterKey::from_bytes([seed; 32])
}

#[test]
fn a_sealed_secret_opens_again() {
    let key = key(1);
    let blob = seal(&key, "conn-1", STATIC_TOKEN, b"secret token");
    assert_eq!(&blob[..4], &KEY_VERSION.to_be_bytes());
    assert_eq!(blob.len(), 4 + NONCE_LEN + b"secret token".len() + 16);
    assert!(!blob.windows(12).any(|w| w == b"secret token"));
    let opened = open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &blob).unwrap();
    assert_eq!(opened.as_slice(), b"secret token");
}

#[test]
fn every_write_takes_a_fresh_nonce() {
    let key = key(1);
    let a = seal(&key, "conn-1", STATIC_TOKEN, b"same");
    let b = seal(&key, "conn-1", STATIC_TOKEN, b"same");
    assert_ne!(a[4..4 + NONCE_LEN], b[4..4 + NONCE_LEN]);
    assert_ne!(a, b);
}

#[test]
fn a_blob_opens_only_for_its_own_row_and_field() {
    let key = key(1);
    let blob = seal(&key, "conn-1", STATIC_TOKEN, b"secret");
    assert_eq!(
        open(&key, "conn-2", STATIC_TOKEN, KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
    assert_eq!(
        open(&key, "conn-1", "oauth_tokens", KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
    // The parts are length-prefixed: moving a character from the id to
    // the field is another AAD.
    let blob = seal(&key, "conn-1x", "y", b"secret");
    assert_eq!(
        open(&key, "conn-1", "xy", KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
}

#[test]
fn another_key_opens_nothing() {
    let blob = seal(&key(1), "conn-1", STATIC_TOKEN, b"secret");
    assert_eq!(
        open(&key(2), "conn-1", STATIC_TOKEN, KEY_VERSION, &blob).unwrap_err(),
        CryptoError::Refused
    );
}

#[test]
fn a_changed_byte_or_version_is_refused() {
    let key = key(1);
    let blob = seal(&key, "conn-1", STATIC_TOKEN, b"secret");
    for at in [4, 4 + NONCE_LEN, blob.len() - 1] {
        let mut tampered = blob.clone();
        tampered[at] ^= 1;
        assert_eq!(
            open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &tampered).unwrap_err(),
            CryptoError::Refused,
            "byte {at}"
        );
    }
    // The blob's version must be the column's, and the key's.
    let mut relabelled = blob.clone();
    relabelled[..4].copy_from_slice(&2u32.to_be_bytes());
    assert_eq!(
        open(&key, "conn-1", STATIC_TOKEN, 2, &relabelled).unwrap_err(),
        CryptoError::KeyVersion { stored: 2 }
    );
    assert_eq!(
        open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &relabelled).unwrap_err(),
        CryptoError::Malformed
    );
    assert_eq!(
        open(&key, "conn-1", STATIC_TOKEN, 2, &blob).unwrap_err(),
        CryptoError::Malformed
    );
}

#[test]
fn a_short_blob_is_malformed() {
    let key = key(1);
    for len in [0, 3, 4 + NONCE_LEN, 4 + NONCE_LEN + 15] {
        // The right version, where there is room for it: only the length
        // is wrong.
        let mut blob = vec![0u8; len];
        if len >= 4 {
            blob[..4].copy_from_slice(&KEY_VERSION.to_be_bytes());
        }
        assert_eq!(
            open(&key, "conn-1", STATIC_TOKEN, KEY_VERSION, &blob).unwrap_err(),
            CryptoError::Malformed,
            "{len}"
        );
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo build --workspace` (no `--locked`: it records the crate and its dependencies in `Cargo.lock`)
Then: `nix develop -c cargo test -p hennery-gateway --locked`
Expected: FAIL to compile: `unresolved import hennery_gateway::crypto` and `hennery_gateway::key`.

- [ ] **Step 3: Write the implementation**

Create `crates/hennery-gateway/src/crypto.rs`:

```rust
//! Credentials at rest (gateway spec §6): XChaCha20-Poly1305, a random
//! 24-byte nonce per write, stored as `key_version ‖ nonce ‖ ciphertext`
//! (the version as 4 bytes, big-endian; the ciphertext ends in the 16-byte
//! tag).
//!
//! The AAD binds a blob to its row, its field and its key version:
//! `connection_id ‖ field ‖ key_version`, each part preceded by its length
//! as 4 bytes, big-endian (plan 8a decision 7), so no two (id, field) pairs
//! give one AAD. A blob moved to another row, read as another field (a
//! static token as an OAuth token, G-14) or relabelled with another version
//! does not open.

use crate::key::MasterKey;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

/// The nonce's length: XChaCha20's 192 bits.
pub const NONCE_LEN: usize = 24;

/// Poly1305's tag, at the end of every ciphertext.
const TAG_LEN: usize = 16;

/// The version prefix: a `u32`, big-endian.
const VERSION_LEN: usize = 4;

/// The AAD's field for a connection's static token (`cred_kind = static`).
/// Fields are named by their table too, so a later table's blobs (OAuth
/// clients, stdio servers' environments) never share one with these.
pub const STATIC_TOKEN: &str = "gw_credentials.static_token";

/// Why a blob did not open. None of them says more than this: a caller
/// names the row.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// Too short, or its version prefix is not the version its row says.
    #[error("the stored credential is malformed")]
    Malformed,
    /// Sealed under a key version this key is not.
    #[error("the stored credential was sealed under key version {stored}, not this master key's")]
    KeyVersion { stored: u32 },
    /// The tag does not check: another key, another row or field, or a
    /// changed byte.
    #[error("the stored credential does not open with this master key")]
    Refused,
}

fn aad(connection_id: &str, field: &str, key_version: u32) -> Vec<u8> {
    let version = key_version.to_be_bytes();
    let mut out = Vec::with_capacity(12 + connection_id.len() + field.len() + version.len());
    for part in [connection_id.as_bytes(), field.as_bytes(), &version] {
        let len = u32::try_from(part.len()).expect("an AAD part under 4 GiB");
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

fn cipher(key: &MasterKey) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new_from_slice(key.bytes()).expect("a 32-byte key")
}

/// Seal `plaintext` for `connection_id`'s `field` under `key`, with a fresh
/// nonce from the operating system's generator.
pub fn seal(key: &MasterKey, connection_id: &str, field: &str, plaintext: &[u8]) -> Vec<u8> {
    let nonce = hennery_kernel::secret::random_bytes::<NONCE_LEN>();
    let payload = Payload {
        msg: plaintext,
        aad: &aad(connection_id, field, key.version()),
    };
    let sealed = cipher(key)
        .encrypt(&XNonce::from(nonce), payload)
        .expect("XChaCha20-Poly1305 seals any message under 256 GiB");
    let mut out = Vec::with_capacity(VERSION_LEN + NONCE_LEN + sealed.len());
    out.extend_from_slice(&key.version().to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

/// Open what `seal` stored for `connection_id`'s `field`. `key_version` is
/// the row's column: the blob's own prefix must agree with it, and both
/// must be `key`'s version.
pub fn open(
    key: &MasterKey,
    connection_id: &str,
    field: &str,
    key_version: u32,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    if blob.len() < VERSION_LEN + NONCE_LEN + TAG_LEN {
        return Err(CryptoError::Malformed);
    }
    let (version, rest) = blob.split_at(VERSION_LEN);
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let prefix = u32::from_be_bytes(version.try_into().expect("4 bytes"));
    if prefix != key_version {
        return Err(CryptoError::Malformed);
    }
    if key_version != key.version() {
        return Err(CryptoError::KeyVersion { stored: key_version });
    }
    let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("24 bytes");
    let payload = Payload {
        msg: sealed,
        aad: &aad(connection_id, field, key_version),
    };
    cipher(key)
        .decrypt(&XNonce::from(nonce), payload)
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::Refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The review's R1: the key version is part of the AAD itself, not only
    /// checked beside it. Sealed for version 1, the same blob does not open
    /// for version 2 under the same key and nonce.
    #[test]
    fn the_key_version_is_bound_into_the_aad() {
        assert_ne!(aad("conn-1", STATIC_TOKEN, 1), aad("conn-1", STATIC_TOKEN, 2));
        let key = MasterKey::from_bytes([3; 32]);
        let nonce = XNonce::from([4u8; NONCE_LEN]);
        let sealed = cipher(&key)
            .encrypt(
                &nonce,
                Payload {
                    msg: b"secret",
                    aad: &aad("conn-1", STATIC_TOKEN, 1),
                },
            )
            .unwrap();
        let reopened = |version| {
            cipher(&key).decrypt(
                &nonce,
                Payload {
                    msg: &sealed,
                    aad: &aad("conn-1", STATIC_TOKEN, version),
                },
            )
        };
        assert_eq!(reopened(1).unwrap(), b"secret");
        assert!(reopened(2).is_err());
    }
}
```

Create `crates/hennery-gateway/src/key.rs`:

```rust
//! The master key (gateway spec §6, kernel spec §10): 32 random bytes that
//! seal every credential the gateway holds. It comes from, in this order:
//! - the environment, `HENNERY_MASTER_KEY`, as 64 hexadecimal digits;
//! - a systemd credential, `$CREDENTIALS_DIRECTORY/hennery-master-key`,
//!   as 32 raw bytes or 64 hexadecimal digits (`LoadCredential=` or
//!   `SetCredential=`);
//! - `<data>/master.key`, 32 raw bytes, created at the first start.
//!
//! Supplying both of the first two is refused: which one sealed the stored
//! credentials would be a guess. A file is never followed if it is a
//! symlink, and `master.key` is refused if anyone but its user can read or
//! write it. Without a key, credentials are unrecoverable, so a
//! `master.key` that is missing while credentials are stored is an error,
//! never a new key (plan 8a decision 6).
//!
//! The key is held in zeroizing memory and never shown: its `Debug` names
//! only its version.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The key file, in the collector's data directory.
pub const KEY_FILE: &str = "master.key";

/// The environment variable that supplies the key, as 64 hexadecimal digits.
/// The host strips it from what its children inherit
/// (`hennery_host::adapter::HOST_SECRET_VARS`).
pub const KEY_ENV: &str = "HENNERY_MASTER_KEY";

/// The systemd credential's name, in `$CREDENTIALS_DIRECTORY`.
pub const CREDENTIAL_NAME: &str = "hennery-master-key";

/// The version every credential is sealed under until `rotate-key`
/// (plan 8g) adds others.
pub const KEY_VERSION: u32 = 1;

const KEY_LEN: usize = 32;

/// The way out when the key is gone for good (plan 8a decision 8): the
/// collector does not start until the key is back, or the credentials only
/// it opens are given up. Each is then set again.
pub const GIVE_UP: &str = "To give the stored gateway credentials up instead, stop the collector and run \
     `sqlite3 <data>/hennery.db 'DELETE FROM gw_credentials'`, then set each connection's credential again.";

/// The master key, wiped from memory when dropped.
pub struct MasterKey {
    version: u32,
    bytes: Zeroizing<[u8; KEY_LEN]>,
}

impl MasterKey {
    /// A key of `KEY_VERSION` from its bytes.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self {
            version: KEY_VERSION,
            bytes: Zeroizing::new(bytes),
        }
    }

    /// The version every blob this key seals carries.
    pub fn version(&self) -> u32 {
        self.version
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// Where a key may come from (see the module's documentation).
pub struct KeySource {
    /// The collector's data directory, which holds `master.key`.
    pub data_dir: PathBuf,
    /// `HENNERY_MASTER_KEY`, if set.
    pub env: Option<Zeroizing<String>>,
    /// `$CREDENTIALS_DIRECTORY`, if set.
    pub credentials_dir: Option<PathBuf>,
}

impl KeySource {
    /// The sources the collector's own environment names.
    pub fn from_env(data_dir: &Path) -> Result<Self> {
        Self::from_vars(
            data_dir,
            std::env::var_os(KEY_ENV),
            std::env::var_os("CREDENTIALS_DIRECTORY"),
        )
    }

    /// The sources `HENNERY_MASTER_KEY` and `$CREDENTIALS_DIRECTORY` name,
    /// as the environment holds them. A key that is set but not text is an
    /// error, not an unset variable: the operator chose it (the review's
    /// R3).
    pub fn from_vars(
        data_dir: &Path,
        env: Option<std::ffi::OsString>,
        credentials_dir: Option<std::ffi::OsString>,
    ) -> Result<Self> {
        let env = match env.map(std::ffi::OsString::into_string) {
            None => None,
            Some(Ok(text)) => Some(Zeroizing::new(text)),
            Some(Err(raw)) => {
                drop(Zeroizing::new(raw.into_encoded_bytes()));
                bail!("{KEY_ENV}: the master key must be 64 hexadecimal digits");
            }
        };
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            env,
            credentials_dir: credentials_dir.map(PathBuf::from),
        })
    }
}

impl std::fmt::Debug for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeySource")
            .field("data_dir", &self.data_dir)
            .field("env", &self.env.as_ref().map(|_| "<set>"))
            .field("credentials_dir", &self.credentials_dir)
            .finish()
    }
}

/// Where the key in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrigin {
    /// Made now, as `<data>/master.key`.
    Created,
    /// `<data>/master.key`, as an earlier start made it.
    File,
    Environment,
    Credential,
}

/// The key from the first source that has one, or a new `master.key` when
/// none has and `ciphertext_exists` is false.
pub fn load_or_create(source: &KeySource, ciphertext_exists: bool) -> Result<(MasterKey, KeyOrigin)> {
    // Only a credential that is not there is absent: one that cannot be
    // looked at is an error, never a reason to make a key file instead.
    let credential = match source.credentials_dir.as_ref().map(|dir| dir.join(CREDENTIAL_NAME)) {
        None => None,
        Some(path) => match path.symlink_metadata() {
            Ok(_) => Some(path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err).with_context(|| format!("look for the credential {}", path.display())),
        },
    };
    let file = source.data_dir.join(KEY_FILE);
    let supplied = match (&source.env, &credential) {
        (Some(_), Some(_)) => {
            bail!("both {KEY_ENV} and the systemd credential {CREDENTIAL_NAME} supply a master key: keep one")
        }
        (Some(hex), None) => Some((from_hex(hex).context(KEY_ENV)?, KeyOrigin::Environment)),
        (None, Some(path)) => Some((read_credential(path)?, KeyOrigin::Credential)),
        (None, None) => None,
    };
    if let Some((key, origin)) = supplied {
        if file.symlink_metadata().is_ok() {
            tracing::warn!(file = %file.display(), "a master key is supplied, so this key file is not used");
        }
        return Ok((key, origin));
    }
    match file.symlink_metadata() {
        Ok(_) => Ok((read_key_file(&file)?, KeyOrigin::File)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if ciphertext_exists {
                bail!(
                    "{} is missing, but gateway credentials are stored that only it can open: restore it from a \
                     backup, or supply it through {KEY_ENV} or the systemd credential {CREDENTIAL_NAME}. {GIVE_UP}",
                    file.display()
                );
            }
            if source.credentials_dir.is_some() {
                tracing::warn!(
                    credential = CREDENTIAL_NAME,
                    "$CREDENTIALS_DIRECTORY is set but holds no master key: making a key file instead"
                );
            }
            Ok((create_key_file(&file)?, KeyOrigin::Created))
        }
        Err(err) => Err(err).with_context(|| format!("read {}", file.display())),
    }
}

/// 64 hexadecimal digits, in any case. The message never quotes the input.
fn from_hex(text: &str) -> Result<MasterKey> {
    let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
    if hex::decode_to_slice(text.trim_end_matches('\n'), bytes.as_mut_slice()).is_err() {
        bail!("the master key must be 64 hexadecimal digits");
    }
    Ok(MasterKey {
        version: KEY_VERSION,
        bytes,
    })
}

/// The file at `path`, never through a symlink, with its metadata from
/// the descriptor itself.
fn open_no_follow(path: &Path) -> Result<(std::fs::File, std::fs::Metadata)> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("open {} (a symlink is refused)", path.display()))?;
    let meta = file.metadata().with_context(|| format!("stat {}", path.display()))?;
    if !meta.file_type().is_file() {
        bail!("{} is not a regular file", path.display());
    }
    Ok((file, meta))
}

/// At most `max` bytes of `file`, in zeroizing memory.
fn read_at_most(file: std::fs::File, path: &Path, max: usize) -> Result<Zeroizing<Vec<u8>>> {
    let mut content = Zeroizing::new(Vec::with_capacity(max + 1));
    file.take(max as u64 + 1)
        .read_to_end(&mut content)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(content)
}

fn read_key_file(path: &Path) -> Result<MasterKey> {
    let (file, meta) = open_no_follow(path)?;
    if meta.mode() & 0o077 != 0 {
        bail!(
            "{} can be read or changed by other users: `chmod 600` it (and consider whether the key leaked)",
            path.display()
        );
    }
    // A second name for the file is a second way to read or replace it
    // that its mode does not show.
    if meta.nlink() != 1 {
        bail!("{} has other hard links: keep one name for the key", path.display());
    }
    let content = read_at_most(file, path, KEY_LEN)?;
    from_raw(&content)
        .ok_or_else(|| anyhow::anyhow!("{} must hold exactly 32 bytes, not {}", path.display(), content.len()))
}

/// Exactly 32 bytes, copied straight into zeroizing memory.
fn from_raw(content: &[u8]) -> Option<MasterKey> {
    if content.len() != KEY_LEN {
        return None;
    }
    let mut bytes = Zeroizing::new([0u8; KEY_LEN]);
    bytes.copy_from_slice(content);
    Some(MasterKey {
        version: KEY_VERSION,
        bytes,
    })
}

/// A systemd credential: systemd owns the file (often root, with an ACL
/// for the service's user), so only access by every other user is refused.
fn read_credential(path: &Path) -> Result<MasterKey> {
    let (file, meta) = open_no_follow(path)?;
    if meta.mode() & 0o007 != 0 {
        bail!("the credential {} can be read by other users", path.display());
    }
    let content = read_at_most(file, path, 2 * KEY_LEN + 1)?;
    if let Some(key) = from_raw(&content) {
        return Ok(key);
    }
    match std::str::from_utf8(&content) {
        Ok(text) if text.trim_end_matches('\n').len() == 2 * KEY_LEN => {
            from_hex(text).with_context(|| path.display().to_string())
        }
        _ => bail!(
            "the credential {} must hold 32 bytes, or 64 hexadecimal digits",
            path.display()
        ),
    }
}

/// A new key at `path`: created exclusively, never through a symlink, and
/// private (0600) whatever the umask. A write cut short leaves a file of
/// the wrong length, which the next start refuses rather than replaces.
fn create_key_file(path: &Path) -> Result<MasterKey> {
    let key = MasterKey {
        version: KEY_VERSION,
        bytes: Zeroizing::new(hennery_kernel::secret::random_bytes::<KEY_LEN>()),
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(key.bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("write {}", path.display()))?;
    // The directory's entry too: without it a crash can lose the file while
    // the database keeps credentials sealed under the key.
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("sync {}", dir.display()))?;
    }
    tracing::info!(file = %path.display(), "created the gateway's master key: back it up with hennery.db");
    Ok(key)
}
```

Replace the whole of `crates/hennery-gateway/src/lib.rs` with:

```rust
//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod crypto;
pub mod key;
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `nix develop -c cargo test -p hennery-gateway --locked`
Expected: PASS, `crypto` 6 tests, `key` 14, and `crypto`'s unit test 1.

- [ ] **Step 5: Revert-probes**

Each change below, made alone and undone, fails the test named (`cargo test -p hennery-gateway --locked --test key` or `--test crypto`):

| Line | Change | Fails |
|---|---|---|
| `read_key_file`'s `meta.mode() & 0o077 != 0` | `false` | `a_key_file_others_can_read_is_refused_and_left_alone` |
| `open_no_follow`'s `O_NOFOLLOW` | dropped | `a_symlinked_or_odd_key_file_is_refused`, `a_bad_systemd_credential_is_refused` |
| `open_no_follow`'s `is_file()` check | `false` | `a_symlinked_or_odd_key_file_is_refused` |
| `load_or_create`'s `if ciphertext_exists` | `false` | `a_missing_key_while_credentials_exist_is_an_error_and_creates_nothing` |
| `create_key_file`'s `.mode(0o600)` | dropped | `a_new_key_is_created_once_private_and_loaded_again` |
| `read_credential`'s `meta.mode() & 0o007 != 0` | `false` | `a_bad_systemd_credential_is_refused` |
| the `(Some(_), Some(_))` arm | removed (the env arm takes both) | `two_supplied_keys_are_refused` |
| `KeySource`'s `Debug` of `env` | the value | `the_key_never_shows_in_debug_output` |
| `aad`'s length prefix | dropped | `a_blob_opens_only_for_its_own_row_and_field` |
| `aad`'s `connection_id`, then its `field` | dropped, each | `a_blob_opens_only_for_its_own_row_and_field` |
| `open`'s `prefix != key_version` | removed | `a_changed_byte_or_version_is_refused` |
| `open`'s `key_version != key.version()` | removed | `a_changed_byte_or_version_is_refused` |
| `seal`'s nonce | `[0u8; NONCE_LEN]` | `every_write_takes_a_fresh_nonce` |
| `open`'s length check | without `TAG_LEN` | `a_short_blob_is_malformed` |
| `aad`'s `&version` | dropped | `crypto::tests::the_key_version_is_bound_into_the_aad` |
| `from_vars`' non-UTF-8 refusal | `None` | `a_key_source_that_cannot_be_read_is_refused_not_skipped` |
| `load_or_create`'s refusal of a credential look-up that fails | `None` | the same |
| `read_key_file`'s `nlink() != 1` | `false` | `a_hard_linked_key_file_is_refused` |
| the missing key's `GIVE_UP` | dropped | `a_missing_key_says_how_to_give_the_credentials_up` |

- [ ] **Step 6: The five checks, then commit**

Run the five checks (Global Constraints). Then:

```bash
git add Cargo.toml Cargo.lock crates/hennery-gateway
git diff --cached --stat
git -c commit.gpgsign=false commit -m "feat(gateway): the master key and credentials at rest"
```

(The tests alone are committed first, after Step 2, as `test(gateway): the master key and credentials at rest`.)

---

### Task 2: Connections, mounts and the static credential in the store

**Files:**
- Modify: `crates/hennery-gateway/Cargo.toml`, `src/lib.rs`, `Cargo.lock`, `crates/hennery-testkit/Cargo.toml`
- Create: `crates/hennery-gateway/src/model.rs`, `src/schema.rs`, `src/store.rs`
- Test: `crates/hennery-gateway/tests/store.rs`, `tests/owner.rs`, `tests/boundary.rs`; `crates/hennery-testkit/tests/owner_filter.rs`

**Interfaces:**
- Consumes: Task 1's `MasterKey`, `seal`, `open`, `STATIC_TOKEN`; `hennery_kernel::db::{open, open_in_memory, kernel_owner, migrate_component}`; the kernel's `hats` and `hosts` tables.
- Produces:
  - `hennery_gateway::model::{CredKind, ConnectionRecord, NewConnection, ConnectionPatch, Change, CredentialChange, MAX_CONNECTIONS, DEFAULT_HEADER, DEFAULT_PREFIX}`; `Change::Done(Box<ConnectionRecord>)`;
  - `hennery_gateway::store::GatewayStore`: `open(&Path)`, `open_in_memory()`, `owner_id()`, `list()`, `connection(id)`, `create(&NewConnection, now) -> Result<Change>`, `update(id, &ConnectionPatch, now) -> Result<Change>`, `delete(id) -> Result<bool>`, `replace_mounts(id, &[String]) -> Result<Change>`, `set_static_credential(id, token, &MasterKey, now) -> Result<CredentialChange>`, `static_credential(id, &MasterKey) -> Result<Option<StaticCredential>>` (`StaticCredential { token: Zeroizing<String>, url, static_header, static_prefix, internal_network }`, one statement), `has_ciphertext() -> Result<bool>`, `check_key(&MasterKey) -> Result<()>`, `purge_hat(hat_id) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Replace the whole of `crates/hennery-gateway/Cargo.toml` with:

```toml
[package]
name = "hennery-gateway"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish.workspace = true

# Never `hennery-sessions`, in any table (umbrella §9, gateway spec §10):
# `tests/boundary.rs` reads the manifests and fails if it appears.
[dependencies]
anyhow.workspace = true
axum.workspace = true
chacha20poly1305.workspace = true
hennery-kernel.workspace = true
hex.workspace = true
libc = "0.2"
rusqlite.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
url.workspace = true
zeroize.workspace = true

[dev-dependencies]
ed25519-dalek.workspace = true
# The store's unit tests read what its statements touch (`hooks`).
rusqlite = { workspace = true, features = ["hooks"] }
tempfile.workspace = true
toml.workspace = true
```

Create `crates/hennery-gateway/tests/store.rs`:

```rust
//! The gateway's store (gateway spec §2, §4.6, §6): connections, their
//! mounts and their static credential, in `hennery.db` beside the kernel's
//! tables, under the kernel's owner.

use ed25519_dalek::SigningKey;
use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{Change, ConnectionPatch, CredKind, CredentialChange, MAX_CONNECTIONS, NewConnection};
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hats::HatChange;
use hennery_kernel::hosts::{Enrollment, Hosts};
use std::path::PathBuf;

const NOW: i64 = 1_800_000_000;
const TOKEN: &str = "ghp_s3cr3tT0kenThatMustNeverLeak";

struct World {
    _dir: tempfile::TempDir,
    db: PathBuf,
    hosts: Hosts,
    store: GatewayStore,
    key: MasterKey,
    hat: String,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let hosts = Hosts::open(&db).unwrap();
        let store = GatewayStore::open(&db).unwrap();
        let hat = hosts.default_hat_for_new_hosts().unwrap();
        Self {
            _dir: dir,
            db,
            hosts,
            store,
            key: MasterKey::from_bytes([7; 32]),
            hat,
        }
    }

    fn hat(&self, name: &str) -> String {
        let HatChange::Done(hat) = self.hosts.create_hat(name, None, NOW).unwrap() else {
            panic!("no hat");
        };
        hat.id
    }

    fn host(&self, id: &str, seed: u8) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let enrollment = Enrollment {
            public_key: hex::encode(key.verifying_key().as_bytes()),
            name: format!("host {seed}"),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        self.hosts.register(id, &enrollment, NOW).unwrap();
    }

    fn new_connection(&self, slug: &str) -> NewConnection {
        NewConnection {
            slug: slug.into(),
            label: "Linear".into(),
            url: "https://mcp.linear.example/sse".into(),
            hat_id: self.hat.clone(),
            cred_kind: CredKind::Static,
            static_header: None,
            static_prefix: None,
            tool_allowlist: None,
            internal_network: false,
        }
    }

    fn create(&self, slug: &str) -> String {
        match self.store.create(&self.new_connection(slug), NOW).unwrap() {
            Change::Done(record) => record.id,
            other => panic!("not created: {other:?}"),
        }
    }

    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db).unwrap()
    }

    fn count(&self, table: &str) -> i64 {
        self.sql()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

fn invalid(change: Change) -> String {
    match change {
        Change::Invalid(why) => why,
        other => panic!("not invalid: {other:?}"),
    }
}

fn done(change: Change) -> hennery_gateway::model::ConnectionRecord {
    match change {
        Change::Done(record) => *record,
        other => panic!("not done: {other:?}"),
    }
}

#[test]
fn a_connection_is_created_with_its_defaults_and_listed() {
    let w = World::new();
    let record = done(w.store.create(&w.new_connection("linear"), NOW).unwrap());
    assert!(record.id.starts_with("conn-") && record.id.len() == 21, "{}", record.id);
    assert_eq!(record.slug, "linear");
    assert_eq!(record.url, "https://mcp.linear.example/sse");
    assert_eq!(record.hat_id, w.hat);
    assert_eq!(record.cred_kind, CredKind::Static);
    assert_eq!(record.static_header, "Authorization");
    assert_eq!(record.static_prefix, "Bearer ");
    assert_eq!(record.tool_allowlist, None);
    assert!(!record.internal_network);
    assert_eq!(record.status, "not_connected");
    assert_eq!(
        (record.status_at, record.created_at, record.updated_at),
        (NOW, NOW, NOW)
    );
    assert!(!record.has_credential);
    assert!(record.mounts.is_empty());
    assert_eq!(w.store.list().unwrap(), vec![record.clone()]);
    assert_eq!(w.store.connection(&record.id).unwrap(), Some(record));
    assert_eq!(w.store.connection("conn-0000000000000000").unwrap(), None);
    // A URL is stored as parsed: an empty path becomes `/`.
    let mut bare = w.new_connection("bare");
    bare.url = "HTTPS://Example.COM".into();
    assert_eq!(done(w.store.create(&bare, NOW).unwrap()).url, "https://example.com/");
}

/// One change to a connection that makes it invalid.
type Edit = Box<dyn Fn(&mut NewConnection)>;

#[test]
fn what_a_connection_is_made_of_is_checked() {
    let w = World::new();
    let base = w.new_connection("ok");
    let cases: Vec<(&str, Edit)> = vec![
        ("slug", Box::new(|c| c.slug = "".into())),
        ("slug", Box::new(|c| c.slug = "-lead".into())),
        ("slug", Box::new(|c| c.slug = "Upper".into())),
        ("slug", Box::new(|c| c.slug = "under_score".into())),
        ("slug", Box::new(|c| c.slug = "a".repeat(49))),
        ("label", Box::new(|c| c.label = " ".into())),
        ("label", Box::new(|c| c.label = "x".repeat(65))),
        ("label", Box::new(|c| c.label = "bidi\u{202e}".into())),
        ("url", Box::new(|c| c.url = "not a url".into())),
        ("url", Box::new(|c| c.url = "ftp://files.example/".into())),
        ("url", Box::new(|c| c.url = "https://user:pass@mcp.example/".into())),
        ("url", Box::new(|c| c.url = "https://user@mcp.example/".into())),
        ("url", Box::new(|c| c.url = "https://mcp.example/#frag".into())),
        (
            "url",
            Box::new(|c| c.url = format!("https://mcp.example/{}", "a".repeat(2048))),
        ),
        ("http", Box::new(|c| c.url = "http://mcp.example/".into())),
        ("header", Box::new(|c| c.static_header = Some("".into()))),
        ("header", Box::new(|c| c.static_header = Some("Bad Header".into()))),
        ("header", Box::new(|c| c.static_header = Some("Host".into()))),
        ("header", Box::new(|c| c.static_header = Some("content-length".into()))),
        ("header", Box::new(|c| c.static_header = Some("Mcp-Session-Id".into()))),
        ("header", Box::new(|c| c.static_header = Some("Cookie".into()))),
        ("header", Box::new(|c| c.static_header = Some("Forwarded".into()))),
        ("header", Box::new(|c| c.static_header = Some("Proxy-Foo".into()))),
        ("prefix", Box::new(|c| c.static_prefix = Some("Bearer\n".into()))),
        ("prefix", Box::new(|c| c.static_prefix = Some("x".repeat(33)))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["".into()]))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["two words".into()]))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["x".repeat(129)]))),
        ("tool", Box::new(|c| c.tool_allowlist = Some(vec!["t".into(); 1025]))),
    ];
    for (about, change) in cases {
        let mut bad = base.clone();
        change(&mut bad);
        let why = invalid(w.store.create(&bad, NOW).unwrap());
        assert!(why.contains(about), "{about}: {why}");
    }
    assert_eq!(w.count("gw_connections"), 0);
    // `http` only to an upstream the operator marked internal (plan 8a
    // decision 9); other headers and prefixes, and the allowlist, as given.
    let mut internal = base.clone();
    internal.url = "http://10.0.0.5:8080/mcp".into();
    internal.internal_network = true;
    internal.static_header = Some("X-API-Key".into());
    internal.static_prefix = Some("".into());
    internal.tool_allowlist = Some(vec!["search".into(), "fetch".into(), "search".into()]);
    let record = done(w.store.create(&internal, NOW).unwrap());
    assert_eq!(record.static_header, "X-API-Key");
    assert_eq!(record.static_prefix, "");
    assert_eq!(record.tool_allowlist, Some(vec!["search".into(), "fetch".into()]));
    assert!(record.internal_network);
}

#[test]
fn a_slug_is_unique_per_owner_and_a_hat_must_be_the_owners() {
    let w = World::new();
    w.create("linear");
    assert!(matches!(
        w.store.create(&w.new_connection("linear"), NOW).unwrap(),
        Change::SlugTaken
    ));
    let mut elsewhere = w.new_connection("other");
    elsewhere.hat_id = "hat-0000000000000000".into();
    assert!(invalid(w.store.create(&elsewhere, NOW).unwrap()).contains("hat"));
    // Another hat of the owner's is fine.
    let mut work = w.new_connection("work");
    work.hat_id = w.hat("Work");
    assert_eq!(done(w.store.create(&work, NOW).unwrap()).hat_id, work.hat_id);
}

#[test]
fn oauth_kinds_wait_for_plan_8f() {
    let w = World::new();
    for kind in [CredKind::OauthDcr, CredKind::OauthClient] {
        let mut oauth = w.new_connection("oauth");
        oauth.cred_kind = kind;
        assert!(matches!(w.store.create(&oauth, NOW).unwrap(), Change::Unsupported(k) if k == kind));
        let id = w.create(&format!("s-{}", kind.as_str().replace('_', "-")));
        let patch = ConnectionPatch {
            cred_kind: Some(kind),
            ..ConnectionPatch::default()
        };
        assert!(matches!(w.store.update(&id, &patch, NOW).unwrap(), Change::Unsupported(k) if k == kind));
    }
    assert_eq!(w.count("gw_connections"), 2);
}

#[test]
fn there_are_at_most_so_many_connections() {
    let w = World::new();
    for i in 0..MAX_CONNECTIONS {
        w.create(&format!("c{i}"));
    }
    assert!(matches!(
        w.store.create(&w.new_connection("one-more"), NOW).unwrap(),
        Change::TooMany
    ));
}

#[test]
fn an_update_changes_what_it_names_and_keeps_the_rest() {
    let w = World::new();
    let mut new = w.new_connection("linear");
    new.tool_allowlist = Some(vec!["search".into()]);
    let before = done(w.store.create(&new, NOW).unwrap());
    let id = before.id.clone();
    // Nothing named: nothing changes but the stamp.
    let same = done(w.store.update(&id, &ConnectionPatch::default(), NOW + 1).unwrap());
    assert_eq!(
        hennery_gateway::model::ConnectionRecord {
            updated_at: NOW,
            ..same.clone()
        },
        before
    );
    assert_eq!(same.updated_at, NOW + 1);
    let renamed = done(
        w.store
            .update(
                &id,
                &ConnectionPatch {
                    label: Some("Linear (work)".into()),
                    static_prefix: Some("".into()),
                    ..ConnectionPatch::default()
                },
                NOW + 2,
            )
            .unwrap(),
    );
    assert_eq!(renamed.label, "Linear (work)");
    assert_eq!(renamed.static_prefix, "");
    assert_eq!(renamed.tool_allowlist, Some(vec!["search".into()]));
    // An explicit `null` clears the allowlist (all tools); `[]` allows none.
    let cleared = ConnectionPatch {
        tool_allowlist: Some(None),
        ..ConnectionPatch::default()
    };
    assert_eq!(done(w.store.update(&id, &cleared, NOW).unwrap()).tool_allowlist, None);
    let none = ConnectionPatch {
        tool_allowlist: Some(Some(vec![])),
        ..ConnectionPatch::default()
    };
    assert_eq!(
        done(w.store.update(&id, &none, NOW).unwrap()).tool_allowlist,
        Some(vec![])
    );
    // Checked as on create, and nothing changed when refused.
    let bad = ConnectionPatch {
        label: Some("".into()),
        ..ConnectionPatch::default()
    };
    assert!(invalid(w.store.update(&id, &bad, NOW).unwrap()).contains("label"));
    assert!(matches!(
        w.store
            .update("conn-0000000000000000", &ConnectionPatch::default(), NOW)
            .unwrap(),
        Change::NotFound
    ));
}

#[test]
fn http_stays_only_while_the_connection_is_internal() {
    let w = World::new();
    let mut new = w.new_connection("lan");
    new.url = "http://10.0.0.5/mcp".into();
    new.internal_network = true;
    let id = done(w.store.create(&new, NOW).unwrap()).id;
    let public = ConnectionPatch {
        internal_network: Some(false),
        ..ConnectionPatch::default()
    };
    assert!(invalid(w.store.update(&id, &public, NOW).unwrap()).contains("http"));
    let both = ConnectionPatch {
        internal_network: Some(false),
        url: Some("https://mcp.example/".into()),
        ..ConnectionPatch::default()
    };
    assert!(!done(w.store.update(&id, &both, NOW).unwrap()).internal_network);
}

/// Gateway spec §4.6 (G-14): another origin or another kind deletes the
/// credential before the change is saved, and the status starts over.
#[test]
fn another_origin_or_kind_deletes_the_credential() {
    let w = World::new();
    let id = w.create("linear");
    let set = || {
        assert_eq!(
            w.store.set_static_credential(&id, TOKEN, &w.key, NOW).unwrap(),
            CredentialChange::Done
        );
        assert!(w.store.connection(&id).unwrap().unwrap().has_credential);
    };
    let has = || w.store.connection(&id).unwrap().unwrap().has_credential;
    set();
    w.sql()
        .execute(
            "UPDATE gw_connections SET status = 'ok', status_note = 'fine', account_label = 'me' WHERE id = ?1",
            [&id],
        )
        .unwrap();
    // The same origin, another path; another header and prefix: kept.
    for patch in [
        ConnectionPatch {
            url: Some("https://mcp.linear.example/v2/mcp?team=1".into()),
            ..ConnectionPatch::default()
        },
        ConnectionPatch {
            static_header: Some("X-API-Key".into()),
            static_prefix: Some("".into()),
            ..ConnectionPatch::default()
        },
        ConnectionPatch {
            cred_kind: Some(CredKind::Static),
            ..ConnectionPatch::default()
        },
    ] {
        let record = done(w.store.update(&id, &patch, NOW + 1).unwrap());
        assert!(record.has_credential, "{patch:?}");
        assert_eq!(record.status, "ok");
    }
    // Another host, scheme or port: deleted, and not connected any more.
    for url in [
        "https://mcp.other.example/v2/mcp",
        "https://mcp.other.example:8443/v2/mcp",
        "http://mcp.other.example:8443/v2/mcp",
    ] {
        set();
        w.sql()
            .execute(
                "UPDATE gw_connections SET status = 'ok', account_label = 'me' WHERE id = ?1",
                [&id],
            )
            .unwrap();
        let patch = ConnectionPatch {
            url: Some(url.into()),
            internal_network: Some(true),
            ..ConnectionPatch::default()
        };
        let record = done(w.store.update(&id, &patch, NOW + 2).unwrap());
        assert!(!record.has_credential, "{url}");
        assert_eq!(
            (
                record.status.as_str(),
                record.status_note,
                record.account_label,
                record.status_at
            ),
            ("not_connected", None, None, NOW + 2)
        );
        assert_eq!(w.count("gw_credentials"), 0);
    }
    // Another kind.
    set();
    let none = ConnectionPatch {
        cred_kind: Some(CredKind::None),
        ..ConnectionPatch::default()
    };
    let record = done(w.store.update(&id, &none, NOW).unwrap());
    assert_eq!(record.cred_kind, CredKind::None);
    assert!(!has());
    assert_eq!(w.count("gw_credentials"), 0);
    // A refused change deletes nothing.
    let back = ConnectionPatch {
        cred_kind: Some(CredKind::Static),
        ..ConnectionPatch::default()
    };
    done(w.store.update(&id, &back, NOW).unwrap());
    set();
    let refused = ConnectionPatch {
        url: Some("http://mcp.elsewhere.example/".into()),
        internal_network: Some(false),
        ..ConnectionPatch::default()
    };
    invalid(w.store.update(&id, &refused, NOW).unwrap());
    assert!(has());
}

#[test]
fn a_static_credential_is_stored_sealed_and_opens_again() {
    let w = World::new();
    let id = w.create("linear");
    assert_eq!(
        w.store.set_static_credential(&id, TOKEN, &w.key, NOW).unwrap(),
        CredentialChange::Done
    );
    // The token with where it goes, from one read (the review's R2).
    let stored = w.store.static_credential(&id, &w.key).unwrap().unwrap();
    assert_eq!(stored.token.as_str(), TOKEN);
    assert_eq!(
        (
            stored.url.as_str(),
            stored.static_header.as_str(),
            stored.static_prefix.as_str()
        ),
        ("https://mcp.linear.example/sse", "Authorization", "Bearer ")
    );
    assert!(!stored.internal_network);
    assert!(!format!("{stored:?}").contains(TOKEN), "{stored:?}");
    // Replaced, not added.
    w.store
        .set_static_credential(&id, "second-token", &w.key, NOW + 1)
        .unwrap();
    assert_eq!(w.count("gw_credentials"), 1);
    assert_eq!(
        w.store.static_credential(&id, &w.key).unwrap().unwrap().token.as_str(),
        "second-token"
    );
    let (version, blob): (i64, Vec<u8>) = w
        .sql()
        .query_row(
            "SELECT key_version, ciphertext FROM gw_credentials WHERE connection_id = ?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(version, 1);
    assert!(!blob.windows(12).any(|w| w == b"second-token"));
    // Another key opens nothing, and says so.
    let other = MasterKey::from_bytes([8; 32]);
    assert!(w.store.static_credential(&id, &other).is_err());
    assert!(w.store.check_key(&w.key).is_ok());
    assert!(w.store.check_key(&other).is_err());
    // A blob moved to another connection does not open there (AAD).
    let moved = w.create("moved");
    w.sql()
        .execute(
            "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, updated_at)
             SELECT ?2, owner_id, key_version, ciphertext, updated_at FROM gw_credentials WHERE connection_id = ?1",
            [&id, &moved],
        )
        .unwrap();
    assert!(w.store.static_credential(&moved, &w.key).is_err());
}

#[test]
fn a_credential_goes_only_to_a_static_connection_and_is_checked() {
    let w = World::new();
    let mut none = w.new_connection("public");
    none.cred_kind = CredKind::None;
    let none = done(w.store.create(&none, NOW).unwrap()).id;
    assert_eq!(
        w.store.set_static_credential(&none, TOKEN, &w.key, NOW).unwrap(),
        CredentialChange::WrongKind(CredKind::None)
    );
    let id = w.create("linear");
    for bad in [
        "",
        "two words",
        "line\nbreak",
        "tab\there",
        "ünïcode",
        &"x".repeat(8193),
    ] {
        assert!(
            matches!(
                w.store.set_static_credential(&id, bad, &w.key, NOW).unwrap(),
                CredentialChange::Invalid(_)
            ),
            "{bad:?}"
        );
    }
    assert_eq!(
        w.store
            .set_static_credential("conn-0000000000000000", TOKEN, &w.key, NOW)
            .unwrap(),
        CredentialChange::NotFound
    );
    assert_eq!(w.count("gw_credentials"), 0);
    assert!(!w.store.has_ciphertext().unwrap());
    w.store.set_static_credential(&id, TOKEN, &w.key, NOW).unwrap();
    assert!(w.store.has_ciphertext().unwrap());
}

#[test]
fn mounts_are_replaced_as_a_set_of_the_owners_live_hosts() {
    let w = World::new();
    w.host("host-a", 1);
    w.host("host-b", 2);
    w.host("host-c", 3);
    let id = w.create("linear");
    let mounts = |hosts: &[&str]| {
        let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
        w.store.replace_mounts(&id, &hosts).unwrap()
    };
    assert_eq!(
        done(mounts(&["host-b", "host-a", "host-b"])).mounts,
        ["host-a", "host-b"]
    );
    assert_eq!(done(mounts(&["host-c"])).mounts, ["host-c"]);
    // An unknown host refuses the whole set.
    assert!(invalid(mounts(&["host-a", "host-x"])).contains("host-x"));
    assert_eq!(w.store.connection(&id).unwrap().unwrap().mounts, ["host-c"]);
    // A revoked one too; and its mounts are no longer listed (plan 8a
    // decision 12), while the row stays until the set is next replaced.
    done(mounts(&["host-a", "host-c"]));
    w.hosts.revoke("host-a", NOW).unwrap();
    assert!(invalid(mounts(&["host-a"])).contains("host-a"));
    assert_eq!(w.store.connection(&id).unwrap().unwrap().mounts, ["host-c"]);
    assert_eq!(w.store.list().unwrap()[0].mounts, ["host-c"]);
    assert_eq!(w.count("gw_mounts"), 2);
    assert!(done(mounts(&[])).mounts.is_empty());
    assert_eq!(w.count("gw_mounts"), 0);
    // Bounded before anything is read; a long id is not echoed back.
    let many: Vec<String> = (0..=hennery_gateway::model::MAX_MOUNTS)
        .map(|i| format!("host-{i}"))
        .collect();
    assert!(invalid(w.store.replace_mounts(&id, &many).unwrap()).contains("at most"));
    let long = "h".repeat(hennery_gateway::model::MAX_HOST_ID + 1);
    let why = invalid(w.store.replace_mounts(&id, std::slice::from_ref(&long)).unwrap());
    assert!(why.contains("at most") && !why.contains(&long), "{why}");
    assert!(matches!(
        w.store.replace_mounts("conn-0000000000000000", &[]).unwrap(),
        Change::NotFound
    ));
}

#[test]
fn a_deleted_connection_takes_its_mounts_and_credential_with_it() {
    let w = World::new();
    w.host("host-a", 1);
    let id = w.create("linear");
    let kept = w.create("kept");
    for connection in [&id, &kept] {
        w.store.replace_mounts(connection, &["host-a".into()]).unwrap();
        w.store.set_static_credential(connection, TOKEN, &w.key, NOW).unwrap();
    }
    assert!(w.store.delete(&id).unwrap());
    assert!(!w.store.delete(&id).unwrap());
    assert_eq!(w.store.connection(&id).unwrap(), None);
    assert_eq!((w.count("gw_mounts"), w.count("gw_credentials")), (1, 1));
    assert!(w.store.connection(&kept).unwrap().unwrap().has_credential);
    // The slug is free again.
    w.create("linear");
}

/// Gateway spec §2, kernel spec §5.5 (lane L6): a hat's purge deletes its
/// connections with their credentials and mounts, nothing of another hat's,
/// and repeats harmlessly; the hat's own row can go after it.
#[test]
fn purging_a_hat_deletes_its_connections_and_repeats_harmlessly() {
    let w = World::new();
    w.host("host-a", 1);
    let work = w.hat("Work");
    let mut in_work = w.new_connection("work-linear");
    in_work.hat_id = work.clone();
    let gone = done(w.store.create(&in_work, NOW).unwrap()).id;
    let kept = w.create("personal-linear");
    for connection in [&gone, &kept] {
        w.store.replace_mounts(connection, &["host-a".into()]).unwrap();
        w.store.set_static_credential(connection, TOKEN, &w.key, NOW).unwrap();
    }
    // The hat cannot go while it has connections (no cascade, L6).
    assert!(w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).is_err());
    w.store.purge_hat(&work).unwrap();
    assert_eq!(w.store.connection(&gone).unwrap(), None);
    assert!(w.store.connection(&kept).unwrap().unwrap().has_credential);
    assert_eq!(
        (
            w.count("gw_connections"),
            w.count("gw_mounts"),
            w.count("gw_credentials")
        ),
        (1, 1, 1)
    );
    w.store.purge_hat(&work).unwrap();
    w.sql().execute("DELETE FROM hats WHERE id = ?1", [&work]).unwrap();
    w.store.purge_hat(&work).unwrap();
    w.store.purge_hat("hat-0000000000000000").unwrap();
    assert_eq!(w.count("gw_connections"), 1);
}

#[test]
fn the_store_and_the_kernel_agree_on_the_owner_whichever_opens_first() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("gateway-first.db");
    let store = GatewayStore::open(&first).unwrap();
    assert_eq!(Hosts::open(&first).unwrap().owner_id(), store.owner_id());
    let last = dir.path().join("gateway-last.db");
    let hosts = Hosts::open(&last).unwrap();
    assert_eq!(GatewayStore::open(&last).unwrap().owner_id(), hosts.owner_id());
    // Opened again, nothing is migrated twice.
    GatewayStore::open(&last).unwrap();
    let version: i64 = rusqlite::Connection::open(&last)
        .unwrap()
        .query_row(
            "SELECT version FROM schema_versions WHERE component = 'gateway'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, 1);
}

/// Plan 8a decision 19 (lane L11): a connection's `Debug` shows only its
/// URL's origin, never the path or query some vendors put a secret in.
#[test]
fn a_connections_debug_shows_only_its_urls_origin() {
    let w = World::new();
    let canary = "c4n4ry-path-secret";
    let mut new = w.new_connection("linear");
    new.url = format!("https://mcp.vendor.example:8443/s/{canary}/mcp?k={canary}");
    let record = done(w.store.create(&new, NOW).unwrap());
    let patch = ConnectionPatch {
        url: Some(new.url.clone()),
        ..ConnectionPatch::default()
    };
    for shown in [format!("{record:?}"), format!("{new:?}"), format!("{patch:?}")] {
        assert!(!shown.contains(canary), "{shown}");
        assert!(shown.contains("https://mcp.vendor.example:8443"), "{shown}");
    }
    assert_eq!(
        hennery_gateway::model::url_for_logs("https://user:pw@h.example/x"),
        "https://h.example"
    );
    assert_eq!(hennery_gateway::model::url_for_logs("not a url"), "<not a url>");
}
```

Create `crates/hennery-gateway/tests/owner.rs`:

```rust
//! `owner_id` in the gateway's store (kernel spec §1, lane L6): every row
//! carries it, and another owner's connections, credentials and mounts are
//! invisible to, and unchanged by, every method of the store.

use hennery_gateway::key::MasterKey;
use hennery_gateway::model::{Change, ConnectionPatch, CredKind, CredentialChange, NewConnection};
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::Hosts;
use rusqlite::types::Value;

/// A second owner, written straight into the database: nothing in v1
/// makes one. Its `created_at` is later than any real clock reaches, so it
/// is never the database's owner (3b-iii review, A5).
const OTHER: &str = "owner-00000000000000b2";
const OTHER_CREATED_AT: i64 = i64::MAX;
const THEIR_HAT: &str = "hat-00000000000000b2";
const THEIR_HOST: &str = "host-00000000000000b2";
const THEIRS: &str = "conn-00000000000000b2";

const TABLES: &[&str] = &["gw_connections", "gw_credentials", "gw_mounts"];

fn rows(conn: &rusqlite::Connection) -> Vec<Vec<Value>> {
    let mut out = Vec::new();
    for table in TABLES {
        let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
        let width = stmt.column_count();
        let found = stmt
            .query_map([], |r| {
                (0..width).map(|i| r.get::<_, Value>(i)).collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        out.extend(found.map(Result::unwrap));
    }
    out
}

/// The other owner, with a hat, a host, and a static connection named
/// `linear` in that hat, credentialled and mounted on that host.
fn write_other_owner(conn: &rusqlite::Connection, key: &MasterKey) {
    conn.execute_batch(&format!(
        "
        INSERT INTO owners(id, created_at, set_up_at) VALUES ('{OTHER}', {OTHER_CREATED_AT}, {OTHER_CREATED_AT});
        INSERT INTO hats(id, owner_id, name, colour, created_at) VALUES ('{THEIR_HAT}', '{OTHER}', 'Theirs', '#000000', 0);
        INSERT INTO hosts(id, owner_id, name, public_key, platform, host_version, default_hat_id, created_at)
            VALUES ('{THEIR_HOST}', '{OTHER}', 'theirs', 'b2', 'linux-x86_64', '0.0.0', '{THEIR_HAT}', 0);
        INSERT INTO gw_connections(id, owner_id, slug, label, url, hat_id, cred_kind, internal_network,
                                   status_at, created_at, updated_at)
            VALUES ('{THEIRS}', '{OTHER}', 'linear', 'Theirs', 'https://theirs.example/', '{THEIR_HAT}', 'static',
                    0, 0, 0, 0);
        INSERT INTO gw_mounts(connection_id, host_id, owner_id) VALUES ('{THEIRS}', '{THEIR_HOST}', '{OTHER}');
        "
    ))
    .unwrap();
    let blob = hennery_gateway::crypto::seal(key, THEIRS, hennery_gateway::crypto::STATIC_TOKEN, b"their-token");
    conn.execute(
        "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, updated_at)
         VALUES (?1, ?2, 1, ?3, 0)",
        rusqlite::params![THEIRS, OTHER, blob],
    )
    .unwrap();
}

#[test]
fn another_owners_connections_are_invisible_to_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let hosts = Hosts::open(&db).unwrap();
    let store = GatewayStore::open(&db).unwrap();
    let key = MasterKey::from_bytes([7; 32]);
    let conn = rusqlite::Connection::open(&db).unwrap();
    write_other_owner(&conn, &key);
    assert!(!store.has_ciphertext().unwrap());
    let before = rows(&conn);

    // Theirs is not found, not changed, not deleted.
    assert!(store.list().unwrap().is_empty());
    assert_eq!(store.connection(THEIRS).unwrap(), None);
    let patch = ConnectionPatch {
        label: Some("Mine now".into()),
        url: Some("https://elsewhere.example/".into()),
        ..ConnectionPatch::default()
    };
    assert!(matches!(store.update(THEIRS, &patch, 1).unwrap(), Change::NotFound));
    assert!(matches!(store.replace_mounts(THEIRS, &[]).unwrap(), Change::NotFound));
    assert_eq!(
        store.set_static_credential(THEIRS, "mine", &key, 1).unwrap(),
        CredentialChange::NotFound
    );
    assert_eq!(store.static_credential(THEIRS, &key).unwrap(), None);
    assert!(!store.delete(THEIRS).unwrap());
    store.purge_hat(THEIR_HAT).unwrap();
    store.check_key(&MasterKey::from_bytes([9; 32])).unwrap();
    assert_eq!(rows(&conn), before);

    // Their hat and host are not the owner's either.
    let new = |slug: &str, hat: &str| NewConnection {
        slug: slug.into(),
        label: "Mine".into(),
        url: "https://mine.example/".into(),
        hat_id: hat.into(),
        cred_kind: CredKind::Static,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    };
    assert!(matches!(
        store.create(&new("mine", THEIR_HAT), 1).unwrap(),
        Change::Invalid(_)
    ));
    assert_eq!(rows(&conn), before);
    // Their slug is the owner's to take (unique per owner, decision 1).
    let hat = hosts.default_hat_for_new_hosts().unwrap();
    let Change::Done(mine) = store.create(&new("linear", &hat), 1).unwrap() else {
        panic!("not created");
    };
    assert!(matches!(
        store.replace_mounts(&mine.id, &[THEIR_HOST.into()]).unwrap(),
        Change::Invalid(_)
    ));
    // The owner's own purge of their own hat leaves theirs alone.
    store.purge_hat(&hat).unwrap();
    assert_eq!(rows(&conn), before);
}
```

Create `crates/hennery-gateway/tests/boundary.rs`:

```rust
//! The module boundary (umbrella §9, gateway spec §10): `hennery-gateway`
//! never depends on `hennery-sessions`, directly or through another crate
//! of the workspace, in any dependency table of its own. Read from the
//! manifests rather than `cargo metadata`, so it holds in a sandboxed
//! build too.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn manifest(path: &Path) -> toml::Table {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    text.parse().unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// The workspace's crates by name, from `[workspace.dependencies]`' paths.
fn workspace_paths(root: &Path) -> std::collections::BTreeMap<String, PathBuf> {
    let workspace = manifest(&root.join("Cargo.toml"));
    workspace["workspace"]["dependencies"]
        .as_table()
        .unwrap()
        .iter()
        .filter_map(|(name, spec)| {
            let path = spec.get("path")?.as_str()?;
            Some((name.clone(), root.join(path)))
        })
        .collect()
}

/// The workspace crates `manifest` names in `tables`, by name.
fn local_deps(
    manifest: &toml::Table,
    tables: &[&str],
    crates: &std::collections::BTreeMap<String, PathBuf>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut sections: Vec<&toml::Table> = vec![manifest];
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        sections.extend(targets.values().filter_map(|t| t.as_table()));
    }
    for section in sections {
        for table in tables {
            let Some(deps) = section.get(*table).and_then(|t| t.as_table()) else {
                continue;
            };
            for (name, spec) in deps {
                let name = spec.get("package").and_then(|p| p.as_str()).unwrap_or(name).to_string();
                if crates.contains_key(&name) || spec.get("path").is_some() {
                    out.push(name);
                }
            }
        }
    }
    out
}

#[test]
fn the_gateway_never_depends_on_the_sessions_module() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = workspace_paths(&root);
    assert!(crates.contains_key("hennery-sessions"), "{crates:?}");
    let gateway = manifest(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
    let mut seen = BTreeSet::new();
    let mut queue = local_deps(
        &gateway,
        &["dependencies", "dev-dependencies", "build-dependencies"],
        &crates,
    );
    assert!(queue.contains(&"hennery-kernel".to_string()), "{queue:?}");
    while let Some(name) = queue.pop() {
        assert_ne!(
            name, "hennery-sessions",
            "hennery-gateway depends on it through {seen:?}"
        );
        if !seen.insert(name.clone()) {
            continue;
        }
        let path = crates
            .get(&name)
            .unwrap_or_else(|| panic!("{name} is not in the workspace"));
        let theirs = manifest(&path.join("Cargo.toml"));
        queue.extend(local_deps(&theirs, &["dependencies", "build-dependencies"], &crates));
    }
    assert!(seen.contains("hennery-proto"), "{seen:?}");
}
```

In `crates/hennery-testkit/Cargo.toml`, replace:

```toml
futures.workspace = true
hennery-host = { workspace = true, features = ["test-hooks"] }
```

with:

```toml
futures.workspace = true
# The owner audit reads the gateway's store too (plan 8a).
hennery-gateway.workspace = true
hennery-host = { workspace = true, features = ["test-hooks"] }
```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
use hennery_kernel::hosts::Hosts;
```

with:

```rust
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::Hosts;
```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        87,
    ),
];
```

with:

```rust
        include_str!("../../hennery-sessions/src/store.rs"),
        87,
    ),
    (
        "hennery-gateway/src/store.rs",
        include_str!("../../hennery-gateway/src/store.rs"),
        GATEWAY_STATEMENTS,
    ),
];

/// The gateway store's statements (plan 8a), apart from the list above so
/// that other lanes' changes to it stay apart from this one.
const GATEWAY_STATEMENTS: usize = 23;
```

In `crates/hennery-testkit/tests/owner_filter.rs`, replace:

```rust
    Hosts::open(&db).unwrap();
    Store::open(&db).unwrap();
    let conn = audit_connection(&db);
```

with:

```rust
    Hosts::open(&db).unwrap();
    Store::open(&db).unwrap();
    GatewayStore::open(&db).unwrap();
    let conn = audit_connection(&db);
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo build --workspace` (no `--locked`: it records the new dependencies in `Cargo.lock`)
Then: `nix develop -c cargo test -p hennery-gateway --locked` and `nix develop -c cargo test -p hennery-testkit --locked --test owner_filter`
Expected: FAIL to compile: `unresolved import hennery_gateway::model` and `hennery_gateway::store`; the audit's `include_str!` finds no `hennery-gateway/src/store.rs`.

- [ ] **Step 3: Write the implementation**

Create `crates/hennery-gateway/src/model.rs`:

```rust
//! A connection as the store keeps it (gateway spec §2), and the rules
//! its parts must meet before anything is written.

use url::Url;

/// The most connections one owner has (plan 8a decision 13).
pub const MAX_CONNECTIONS: usize = 256;

/// The longest URL accepted, in bytes.
pub const MAX_URL: usize = 2048;

/// The longest static token accepted, in bytes.
pub const MAX_TOKEN: usize = 8192;

/// The most tools an allowlist names.
pub const MAX_TOOLS: usize = 1024;

/// The most hosts one mounts request names, and the longest host id it
/// takes, both checked before anything is read.
pub const MAX_MOUNTS: usize = 1024;
pub const MAX_HOST_ID: usize = 64;

/// The header a static token goes in unless the connection names another
/// (maintainer decision 6d), and the prefix before it.
pub const DEFAULT_HEADER: &str = "Authorization";
pub const DEFAULT_PREFIX: &str = "Bearer ";

/// Headers a static token may not be sent in: the ones the proxy itself
/// sets, frames or filters (gateway spec §5.2), and the cookie.
const RESERVED_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "content-type",
    "content-encoding",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "trailer",
    "proxy-authorization",
    "proxy-connection",
    "cookie",
    "accept",
    "accept-encoding",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
    "expect",
    "forwarded",
    "via",
    "max-forwards",
];

/// What of an upstream URL may be shown in a log line, an error or a
/// `Debug` (plan 8a decision 19): `scheme://host[:port]`, never its path or
/// query, which some vendors put a secret in.
pub fn url_for_logs(url: &str) -> String {
    match Url::parse(url) {
        Ok(url) => url.origin().ascii_serialization(),
        Err(_) => "<not a url>".into(),
    }
}

/// How a connection authenticates to its upstream (gateway spec §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredKind {
    None,
    Static,
    OauthDcr,
    OauthClient,
}

impl CredKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Static => "static",
            Self::OauthDcr => "oauth_dcr",
            Self::OauthClient => "oauth_client",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "none" => Self::None,
            "static" => Self::Static,
            "oauth_dcr" => Self::OauthDcr,
            "oauth_client" => Self::OauthClient,
            _ => return None,
        })
    }

    /// Whether this plan's store takes it: OAuth comes with plan 8f
    /// (plan 8a decision 10).
    pub fn is_supported(self) -> bool {
        matches!(self, Self::None | Self::Static)
    }
}

/// One stored connection, without its secret: whether it has one is all
/// that is read of `gw_credentials` (plan 8a decision 3).
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionRecord {
    pub id: String,
    pub slug: String,
    pub label: String,
    /// As parsed and serialised (`url::Url`).
    pub url: String,
    pub hat_id: String,
    pub cred_kind: CredKind,
    pub static_header: String,
    pub static_prefix: String,
    /// `None`: every tool.
    pub tool_allowlist: Option<Vec<String>>,
    pub internal_network: bool,
    /// `not_connected`, `ok`, `needs_auth` or `error` (gateway spec §7).
    pub status: String,
    pub status_note: Option<String>,
    pub account_label: Option<String>,
    pub status_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub has_credential: bool,
    /// The hosts it is mounted on that are not revoked, by id.
    pub mounts: Vec<String>,
}

/// A connection to create. `None` header or prefix: the default.
#[derive(Clone, PartialEq, Eq)]
pub struct NewConnection {
    pub slug: String,
    pub label: String,
    pub url: String,
    pub hat_id: String,
    pub cred_kind: CredKind,
    pub static_header: Option<String>,
    pub static_prefix: Option<String>,
    pub tool_allowlist: Option<Vec<String>>,
    pub internal_network: bool,
}

/// A change to a connection: a field left `None` keeps its value. The
/// allowlist is `Some(None)` to clear it (every tool) and `Some(Some(…))`
/// to set it (gateway spec §4.6). A connection's slug and hat never change
/// (plan 8a decision 5).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConnectionPatch {
    pub label: Option<String>,
    pub url: Option<String>,
    pub cred_kind: Option<CredKind>,
    pub static_header: Option<String>,
    pub static_prefix: Option<String>,
    pub tool_allowlist: Option<Option<Vec<String>>>,
    pub internal_network: Option<bool>,
}

// `Debug` by hand: the URL shows only its origin (decision 19).
impl std::fmt::Debug for ConnectionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionRecord")
            .field("id", &self.id)
            .field("slug", &self.slug)
            .field("label", &self.label)
            .field("url", &url_for_logs(&self.url))
            .field("hat_id", &self.hat_id)
            .field("cred_kind", &self.cred_kind)
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("internal_network", &self.internal_network)
            .field("status", &self.status)
            .field("has_credential", &self.has_credential)
            .field("mounts", &self.mounts)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for NewConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewConnection")
            .field("slug", &self.slug)
            .field("label", &self.label)
            .field("url", &url_for_logs(&self.url))
            .field("hat_id", &self.hat_id)
            .field("cred_kind", &self.cred_kind)
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("internal_network", &self.internal_network)
            .finish()
    }
}

impl std::fmt::Debug for ConnectionPatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPatch")
            .field("label", &self.label)
            .field("url", &self.url.as_deref().map(url_for_logs))
            .field("cred_kind", &self.cred_kind)
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("tool_allowlist", &self.tool_allowlist)
            .field("internal_network", &self.internal_network)
            .finish()
    }
}

/// A `static` connection's token with where it goes, read in one statement
/// (the review's R2): the proxy (plan 8d) must take all four from here, so
/// an edit that moves the URL cannot land between reading the token and
/// reading where to send it. Its `Debug` shows neither the token nor more
/// of the URL than its origin.
#[derive(Clone, PartialEq, Eq)]
pub struct StaticCredential {
    pub token: zeroize::Zeroizing<String>,
    pub url: String,
    pub static_header: String,
    pub static_prefix: String,
    pub internal_network: bool,
}

impl std::fmt::Debug for StaticCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticCredential")
            .field("token", &"<redacted>")
            .field("url", &url_for_logs(&self.url))
            .field("static_header", &self.static_header)
            .field("static_prefix", &self.static_prefix)
            .field("internal_network", &self.internal_network)
            .finish()
    }
}

/// The outcome of a change to a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Done(Box<ConnectionRecord>),
    NotFound,
    /// Another connection of the owner's has this slug.
    SlugTaken,
    /// The owner has `MAX_CONNECTIONS` already.
    TooMany,
    /// A kind this plan does not take yet.
    Unsupported(CredKind),
    /// Why it was refused; nothing was written.
    Invalid(String),
}

/// The outcome of setting a static credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialChange {
    Done,
    NotFound,
    /// The connection's kind takes no static token.
    WrongKind(CredKind),
    Invalid(String),
}

/// `^[a-z0-9][a-z0-9-]{0,47}$` (gateway spec §2).
pub fn slug_problem(slug: &str) -> Option<String> {
    let bytes = slug.as_bytes();
    let ok = (1..=48).contains(&bytes.len())
        && bytes[0] != b'-'
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-');
    (!ok).then(|| "a slug is 1 to 48 of a-z, 0-9 and -, not starting with -".into())
}

/// 1 to 64 bytes once trimmed, nothing invisible or controlling.
pub fn label_problem(label: &str) -> Option<String> {
    let label = label.trim();
    let ok = !label.is_empty() && hennery_kernel::hosts::is_displayable_text(label, 64);
    (!ok).then(|| "a label is 1 to 64 printable characters".into())
}

/// The upstream URL, parsed: `http` or `https`, absolute, without a user
/// name, password or fragment; `http` only to an upstream marked internal
/// (plan 8a decision 9). The URL is listed and logged: secrets go in the
/// credential.
pub fn parse_url(input: &str, internal_network: bool) -> Result<Url, String> {
    if input.len() > MAX_URL {
        return Err(format!("a url is at most {MAX_URL} bytes"));
    }
    let url = Url::parse(input).map_err(|_| "the url is not an absolute http or https URL".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if internal_network => {}
        "http" => return Err("an http url needs internal_network: the token would cross the network in clear".into()),
        _ => return Err("the url is not an absolute http or https URL".into()),
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the url names no host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the url may not hold a user name or password: set a credential".into());
    }
    if url.fragment().is_some() {
        return Err("the url may not have a fragment".into());
    }
    if url.as_str().len() > MAX_URL {
        return Err(format!("a url is at most {MAX_URL} bytes"));
    }
    Ok(url)
}

/// A header name the proxy can send a token in.
pub fn header_problem(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > 64 || axum::http::HeaderName::from_bytes(name.as_bytes()).is_err() {
        return Some("the static header is not a header name".into());
    }
    let lower = name.to_ascii_lowercase();
    if RESERVED_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") || lower.starts_with("sec-") {
        return Some(format!("the static header may not be {name}"));
    }
    None
}

/// At most 32 visible ASCII characters or spaces.
pub fn prefix_problem(prefix: &str) -> Option<String> {
    let ok = prefix.len() <= 32 && prefix.bytes().all(|b| (0x20..=0x7e).contains(&b));
    (!ok).then(|| "the static prefix is at most 32 visible ASCII characters or spaces".into())
}

/// 1 to `MAX_TOKEN` visible ASCII characters: no space, nothing a header
/// could break on.
pub fn token_problem(token: &str) -> Option<String> {
    let ok = (1..=MAX_TOKEN).contains(&token.len()) && token.bytes().all(|b| (0x21..=0x7e).contains(&b));
    (!ok).then(|| format!("a token is 1 to {MAX_TOKEN} visible ASCII characters, without spaces"))
}

/// The allowlist without duplicates, in its order, if each tool is 1 to
/// 128 visible ASCII characters and there are at most `MAX_TOOLS`.
pub fn tools(list: &[String]) -> Result<Vec<String>, String> {
    if list.len() > MAX_TOOLS {
        return Err(format!("a tool allowlist names at most {MAX_TOOLS} tools"));
    }
    let mut out: Vec<String> = Vec::with_capacity(list.len());
    for tool in list {
        if !(1..=128).contains(&tool.len()) || !tool.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return Err("a tool name is 1 to 128 visible ASCII characters".into());
        }
        if !out.contains(tool) {
            out.push(tool.clone());
        }
    }
    Ok(out)
}
```

Create `crates/hennery-gateway/src/schema.rs`:

```rust
//! The gateway's tables in `hennery.db` (gateway spec §2), migrated as a
//! component of their own (`db::migrate_component`), beside the kernel's
//! and the sessions store's. Plan 8a makes the first three; later plans add
//! the session tokens, standalone clients, OAuth clients and stdio servers.
//!
//! Every table carries `owner_id` (lane L6). A connection's hat and a
//! mount's host are the owner's by composite foreign keys (plan 5a decision
//! 3), and a credential's or mount's connection likewise. None cascades:
//! the store deletes a connection's credential and mounts itself, and a hat
//! with connections cannot be deleted until the gateway's purge has run
//! (kernel spec §5.5). The `CHECK`s name every value the spec has, OAuth
//! kinds included: these tables will have children, and a table with
//! children cannot be rebuilt by this runner to widen one later.

/// The gateway's component name in `schema_versions`.
pub(crate) const COMPONENT: &str = "gateway";

pub(crate) const MIGRATIONS: &[&str] = &["
    CREATE TABLE gw_connections (
        id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL REFERENCES owners(id),
        slug TEXT NOT NULL,
        label TEXT NOT NULL,
        url TEXT NOT NULL,
        hat_id TEXT NOT NULL,
        cred_kind TEXT NOT NULL CHECK (cred_kind IN ('none', 'static', 'oauth_dcr', 'oauth_client')),
        static_header TEXT NOT NULL DEFAULT 'Authorization',
        static_prefix TEXT NOT NULL DEFAULT 'Bearer ',
        tool_allowlist TEXT,
        internal_network INTEGER NOT NULL CHECK (internal_network IN (0, 1)),
        status TEXT NOT NULL DEFAULT 'not_connected'
            CHECK (status IN ('not_connected', 'ok', 'needs_auth', 'error')),
        status_note TEXT,
        account_label TEXT,
        status_at INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        UNIQUE (owner_id, slug),
        UNIQUE (id, owner_id),
        FOREIGN KEY (hat_id, owner_id) REFERENCES hats(id, owner_id));
    CREATE INDEX gw_connections_by_hat ON gw_connections(owner_id, hat_id);
    CREATE TABLE gw_credentials (
        connection_id TEXT PRIMARY KEY,
        owner_id TEXT NOT NULL,
        key_version INTEGER NOT NULL,
        ciphertext BLOB NOT NULL,
        expires_at INTEGER,
        updated_at INTEGER NOT NULL,
        FOREIGN KEY (connection_id, owner_id) REFERENCES gw_connections(id, owner_id));
    CREATE TABLE gw_mounts (
        connection_id TEXT NOT NULL,
        host_id TEXT NOT NULL,
        owner_id TEXT NOT NULL,
        PRIMARY KEY (connection_id, host_id),
        FOREIGN KEY (connection_id, owner_id) REFERENCES gw_connections(id, owner_id),
        FOREIGN KEY (host_id, owner_id) REFERENCES hosts(id, owner_id));
    CREATE INDEX gw_mounts_by_host ON gw_mounts(owner_id, host_id);
    "];
```

Create `crates/hennery-gateway/src/store.rs`:

```rust
//! The gateway's store (gateway spec §2): connections, the hosts they are
//! mounted on, and their credentials, sealed (`crypto`). It keeps its own
//! connection to `hennery.db`, beside the kernel's and the sessions
//! store's, and every query names the database's owner (kernel spec §1).
//! Every SQL statement of the gateway is in this file, which the owner
//! audit reads (`hennery-testkit/tests/owner_filter.rs`).
//!
//! What lists a connection reads of `gw_credentials` only whether a row is
//! there (plan 8a decision 3): never the ciphertext, nor its version.

use crate::crypto::{self, STATIC_TOKEN};
use crate::key::MasterKey;
use crate::model::{
    Change, ConnectionPatch, ConnectionRecord, CredKind, CredentialChange, DEFAULT_HEADER, DEFAULT_PREFIX,
    MAX_CONNECTIONS, MAX_HOST_ID, MAX_MOUNTS, NewConnection, StaticCredential, header_problem, label_problem,
    parse_url, prefix_problem, slug_problem, token_problem, tools,
};
use crate::schema::{COMPONENT, MIGRATIONS};
use anyhow::{Context, Result, anyhow};
use hennery_kernel::db;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use zeroize::Zeroizing;

/// A connection's columns, then whether it has a credential: `?1` is the
/// owner, `?2` one connection's id or `NULL` for all of them.
const SELECT_CONNECTIONS: &str = "SELECT c.id, c.slug, c.label, c.url, c.hat_id, c.cred_kind, c.static_header,
            c.static_prefix, c.tool_allowlist, c.internal_network, c.status, c.status_note, c.account_label,
            c.status_at, c.created_at, c.updated_at,
            EXISTS (SELECT 1 FROM gw_credentials k WHERE k.connection_id = c.id AND k.owner_id = ?1)
     FROM gw_connections c
     WHERE c.owner_id = ?1 AND (?2 IS NULL OR c.id = ?2)
     ORDER BY c.created_at, c.id";

/// The mounts on hosts that are not revoked (plan 8a decision 12): `?1`
/// is the owner, `?2` one connection's id or `NULL` for all of them.
const SELECT_MOUNTS: &str = "SELECT m.connection_id, m.host_id
     FROM gw_mounts m JOIN hosts h ON h.id = m.host_id AND h.owner_id = ?1
     WHERE m.owner_id = ?1 AND h.revoked_at IS NULL AND (?2 IS NULL OR m.connection_id = ?2)
     ORDER BY m.connection_id, m.host_id";

pub struct GatewayStore {
    conn: Mutex<Connection>,
    /// The database's owner (`db::kernel_owner`), whom every query names.
    owner: String,
}

impl GatewayStore {
    /// Open the gateway's tables in `hennery.db`, migrating the kernel's
    /// first (the gateway's reference them) and then its own.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(db::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(db::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        let owner = db::kernel_owner(&mut conn)?;
        db::migrate_component(&mut conn, COMPONENT, MIGRATIONS)?;
        Ok(Self {
            conn: Mutex::new(conn),
            owner,
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("gateway store lock")
    }

    /// The owner whose connections these are.
    pub fn owner_id(&self) -> &str {
        &self.owner
    }

    /// Every connection, oldest first.
    pub fn list(&self) -> Result<Vec<ConnectionRecord>> {
        read(&self.conn(), &self.owner, None)
    }

    pub fn connection(&self, id: &str) -> Result<Option<ConnectionRecord>> {
        Ok(read(&self.conn(), &self.owner, Some(id))?.pop())
    }

    /// A new connection (gateway spec §1), not connected, mounted nowhere.
    pub fn create(&self, new: &NewConnection, now: i64) -> Result<Change> {
        if let Some(problem) = slug_problem(&new.slug).or_else(|| label_problem(&new.label)) {
            return Ok(Change::Invalid(problem));
        }
        if !new.cred_kind.is_supported() {
            return Ok(Change::Unsupported(new.cred_kind));
        }
        let url = match parse_url(&new.url, new.internal_network) {
            Ok(url) => url,
            Err(why) => return Ok(Change::Invalid(why)),
        };
        let header = new.static_header.as_deref().unwrap_or(DEFAULT_HEADER);
        let prefix = new.static_prefix.as_deref().unwrap_or(DEFAULT_PREFIX);
        if let Some(problem) = header_problem(header).or_else(|| prefix_problem(prefix)) {
            return Ok(Change::Invalid(problem));
        }
        let allowlist = match new.tool_allowlist.as_deref().map(tools).transpose() {
            Ok(list) => list,
            Err(why) => return Ok(Change::Invalid(why)),
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let hat = tx
            .query_row(
                "SELECT 1 FROM hats WHERE id = ?1 AND owner_id = ?2",
                [&new.hat_id, &self.owner],
                |_| Ok(()),
            )
            .optional()?;
        if hat.is_none() {
            return Ok(Change::Invalid(format!("no hat {:?}", new.hat_id)));
        }
        let taken = tx
            .query_row(
                "SELECT 1 FROM gw_connections WHERE slug = ?1 AND owner_id = ?2",
                [&new.slug, &self.owner],
                |_| Ok(()),
            )
            .optional()?;
        if taken.is_some() {
            return Ok(Change::SlugTaken);
        }
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM gw_connections WHERE owner_id = ?1",
            [&self.owner],
            |r| r.get(0),
        )?;
        if count >= MAX_CONNECTIONS as i64 {
            return Ok(Change::TooMany);
        }
        let id = format!("conn-{}", hex::encode(hennery_kernel::secret::random_bytes::<8>()));
        tx.execute(
            "INSERT INTO gw_connections(id, owner_id, slug, label, url, hat_id, cred_kind, static_header,
                                        static_prefix, tool_allowlist, internal_network, status_at, created_at,
                                        updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, ?12)",
            params![
                id,
                self.owner,
                new.slug,
                new.label.trim(),
                url.as_str(),
                new.hat_id,
                new.cred_kind.as_str(),
                header,
                prefix,
                allowlist.map(|list| serde_json::to_string(&list)).transpose()?,
                new.internal_network,
                now,
            ],
        )?;
        let created = read(&tx, &self.owner, Some(&id))?.pop();
        tx.commit()?;
        Ok(created.map_or(Change::NotFound, |record| Change::Done(Box::new(record))))
    }

    /// Change what `patch` names (gateway spec §4.6). Another origin or
    /// another kind deletes the connection's credential first, in the same
    /// transaction, and its status starts over: the token was granted for
    /// the old upstream and the old kind (G-14).
    pub fn update(&self, id: &str, patch: &ConnectionPatch, now: i64) -> Result<Change> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let Some(current) = read(&tx, &self.owner, Some(id))?.pop() else {
            return Ok(Change::NotFound);
        };
        let label = patch.label.as_deref().unwrap_or(&current.label);
        let kind = patch.cred_kind.unwrap_or(current.cred_kind);
        let internal_network = patch.internal_network.unwrap_or(current.internal_network);
        let header = patch.static_header.as_deref().unwrap_or(&current.static_header);
        let prefix = patch.static_prefix.as_deref().unwrap_or(&current.static_prefix);
        if let Some(problem) = label_problem(label) {
            return Ok(Change::Invalid(problem));
        }
        if !kind.is_supported() {
            return Ok(Change::Unsupported(kind));
        }
        let url = match parse_url(patch.url.as_deref().unwrap_or(&current.url), internal_network) {
            Ok(url) => url,
            Err(why) => return Ok(Change::Invalid(why)),
        };
        if let Some(problem) = header_problem(header).or_else(|| prefix_problem(prefix)) {
            return Ok(Change::Invalid(problem));
        }
        let allowlist = match &patch.tool_allowlist {
            None => current.tool_allowlist.clone(),
            Some(None) => None,
            Some(Some(list)) => match tools(list) {
                Ok(list) => Some(list),
                Err(why) => return Ok(Change::Invalid(why)),
            },
        };
        let old_origin = url::Url::parse(&current.url).context("a stored url")?.origin();
        if url.origin() != old_origin || kind != current.cred_kind {
            tx.execute(
                "DELETE FROM gw_credentials WHERE connection_id = ?1 AND owner_id = ?2",
                [id, &self.owner],
            )?;
            tx.execute(
                "UPDATE gw_connections SET status = 'not_connected', status_note = NULL, account_label = NULL,
                                           status_at = ?3
                 WHERE id = ?1 AND owner_id = ?2",
                params![id, self.owner, now],
            )?;
        }
        tx.execute(
            "UPDATE gw_connections SET label = ?3, url = ?4, cred_kind = ?5, static_header = ?6, static_prefix = ?7,
                                       tool_allowlist = ?8, internal_network = ?9, updated_at = ?10
             WHERE id = ?1 AND owner_id = ?2",
            params![
                id,
                self.owner,
                label.trim(),
                url.as_str(),
                kind.as_str(),
                header,
                prefix,
                allowlist.map(|list| serde_json::to_string(&list)).transpose()?,
                internal_network,
                now,
            ],
        )?;
        let updated = read(&tx, &self.owner, Some(id))?.pop();
        tx.commit()?;
        Ok(updated.map_or(Change::NotFound, |record| Change::Done(Box::new(record))))
    }

    /// Delete a connection with its credential and mounts. False if there
    /// was none.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM gw_credentials WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_mounts WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        let deleted = tx.execute(
            "DELETE FROM gw_connections WHERE id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        tx.commit()?;
        Ok(deleted == 1)
    }

    /// Replace the hosts a connection is mounted on (gateway spec §9: the
    /// full set, never a delta). Each must be the owner's and not revoked,
    /// or nothing changes.
    pub fn replace_mounts(&self, id: &str, host_ids: &[String]) -> Result<Change> {
        // Bounded before the transaction: each id is a query under the lock.
        if host_ids.len() > MAX_MOUNTS {
            return Ok(Change::Invalid(format!(
                "a connection is mounted on at most {MAX_MOUNTS} hosts"
            )));
        }
        if host_ids.iter().any(|host| host.len() > MAX_HOST_ID) {
            return Ok(Change::Invalid(format!("a host id is at most {MAX_HOST_ID} bytes")));
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if read(&tx, &self.owner, Some(id))?.is_empty() {
            return Ok(Change::NotFound);
        }
        let mut hosts: Vec<&String> = host_ids.iter().collect();
        hosts.sort();
        hosts.dedup();
        for host in &hosts {
            let live = tx
                .query_row(
                    "SELECT 1 FROM hosts WHERE id = ?1 AND owner_id = ?2 AND revoked_at IS NULL",
                    [host.as_str(), &self.owner],
                    |_| Ok(()),
                )
                .optional()?;
            if live.is_none() {
                return Ok(Change::Invalid(format!("no host {host:?}, or it is revoked")));
            }
        }
        tx.execute(
            "DELETE FROM gw_mounts WHERE connection_id = ?1 AND owner_id = ?2",
            [id, &self.owner],
        )?;
        for host in hosts {
            tx.execute(
                "INSERT INTO gw_mounts(connection_id, host_id, owner_id) VALUES (?1, ?2, ?3)",
                [id, host.as_str(), &self.owner],
            )?;
        }
        let mounted = read(&tx, &self.owner, Some(id))?.pop();
        tx.commit()?;
        Ok(mounted.map_or(Change::NotFound, |record| Change::Done(Box::new(record))))
    }

    /// Store `token` as the connection's static credential, sealed under
    /// `key`, replacing any before it. Only a `static` connection takes one.
    pub fn set_static_credential(&self, id: &str, token: &str, key: &MasterKey, now: i64) -> Result<CredentialChange> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let kind: Option<String> = tx
            .query_row(
                "SELECT cred_kind FROM gw_connections WHERE id = ?1 AND owner_id = ?2",
                [id, &self.owner],
                |r| r.get(0),
            )
            .optional()?;
        let Some(kind) = kind else {
            return Ok(CredentialChange::NotFound);
        };
        let kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        if kind != CredKind::Static {
            return Ok(CredentialChange::WrongKind(kind));
        }
        if let Some(problem) = token_problem(token) {
            return Ok(CredentialChange::Invalid(problem));
        }
        let sealed = crypto::seal(key, id, STATIC_TOKEN, token.as_bytes());
        tx.execute(
            "INSERT INTO gw_credentials(connection_id, owner_id, key_version, ciphertext, expires_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)
             ON CONFLICT(connection_id) DO UPDATE SET key_version = excluded.key_version,
                 ciphertext = excluded.ciphertext, expires_at = NULL, updated_at = excluded.updated_at
                 WHERE gw_credentials.owner_id = excluded.owner_id",
            params![id, self.owner, key.version(), sealed, now],
        )?;
        tx.commit()?;
        Ok(CredentialChange::Done)
    }

    /// A `static` connection's token, opened with `key`, with where it goes,
    /// all read in one statement (`StaticCredential`); `None` without one.
    /// An error if it does not open: another key, or a blob moved from
    /// another row.
    pub fn static_credential(&self, id: &str, key: &MasterKey) -> Result<Option<StaticCredential>> {
        type Row = (u32, Vec<u8>, String, String, String, String, bool);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT k.key_version, k.ciphertext, c.cred_kind, c.url, c.static_header, c.static_prefix,
                        c.internal_network
                 FROM gw_credentials k JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
                 WHERE k.connection_id = ?1 AND k.owner_id = ?2",
                [id, &self.owner],
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
        let Some((version, blob, kind, url, static_header, static_prefix, internal_network)) = row else {
            return Ok(None);
        };
        if kind != CredKind::Static.as_str() {
            return Ok(None);
        }
        let mut opened =
            crypto::open(key, id, STATIC_TOKEN, version, &blob).with_context(|| format!("connection {id}"))?;
        // Moved out of the zeroizing buffer, not copied; wiped on the error
        // path too.
        let token = match String::from_utf8(std::mem::take(&mut *opened)) {
            Ok(token) => Zeroizing::new(token),
            Err(err) => {
                drop(Zeroizing::new(err.into_bytes()));
                return Err(anyhow!("connection {id}: not a token"));
            }
        };
        Ok(Some(StaticCredential {
            token,
            url,
            static_header,
            static_prefix,
            internal_network,
        }))
    }

    /// Whether any credential is stored: then a missing master key is an
    /// error, never a new key (`key::load_or_create`).
    pub fn has_ciphertext(&self) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM gw_credentials WHERE owner_id = ?1)",
            [&self.owner],
            |r| r.get(0),
        )?)
    }

    /// That `key` opens the newest stored credential, so a wrong key stops
    /// the start instead of sealing new rows beside ones it cannot open
    /// (plan 8a decision 8). Only the newest: a damaged older row shows when
    /// it is used, and does not stop the start.
    pub fn check_key(&self, key: &MasterKey) -> Result<()> {
        let row: Option<(String, u32, Vec<u8>, String)> = self
            .conn()
            .query_row(
                "SELECT k.connection_id, k.key_version, k.ciphertext, c.cred_kind
                 FROM gw_credentials k JOIN gw_connections c ON c.id = k.connection_id AND c.owner_id = k.owner_id
                 WHERE k.owner_id = ?1
                 ORDER BY k.updated_at DESC, k.connection_id LIMIT 1",
                [&self.owner],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
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
                     sealed with. {}",
                    crate::key::GIVE_UP
                )
            })
    }

    /// The gateway's part of purging a hat (kernel spec §5.5, lane L6): the
    /// hat's connections, with their credentials and mounts, in one
    /// transaction. Idempotent: a hat with nothing left, or gone, is done.
    pub fn purge_hat(&self, hat_id: &str) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM gw_credentials WHERE owner_id = ?2
                 AND connection_id IN (SELECT id FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2)",
            [hat_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_mounts WHERE owner_id = ?2
                 AND connection_id IN (SELECT id FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2)",
            [hat_id, &self.owner],
        )?;
        tx.execute(
            "DELETE FROM gw_connections WHERE hat_id = ?1 AND owner_id = ?2",
            [hat_id, &self.owner],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// `owner`'s connections, or the one with `id`, with their live mounts.
fn read(conn: &Connection, owner: &str, id: Option<&str>) -> Result<Vec<ConnectionRecord>> {
    let mut stmt = conn.prepare(SELECT_CONNECTIONS)?;
    let rows = stmt.query_map(params![owner, id], |r| {
        Ok((
            ConnectionRecord {
                id: r.get(0)?,
                slug: r.get(1)?,
                label: r.get(2)?,
                url: r.get(3)?,
                hat_id: r.get(4)?,
                cred_kind: CredKind::None,
                static_header: r.get(6)?,
                static_prefix: r.get(7)?,
                tool_allowlist: None,
                internal_network: r.get(9)?,
                status: r.get(10)?,
                status_note: r.get(11)?,
                account_label: r.get(12)?,
                status_at: r.get(13)?,
                created_at: r.get(14)?,
                updated_at: r.get(15)?,
                has_credential: r.get(16)?,
                mounts: Vec::new(),
            },
            r.get::<_, String>(5)?,
            r.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (mut record, kind, allowlist) = row?;
        record.cred_kind = CredKind::parse(&kind).ok_or_else(|| anyhow!("a stored credential kind"))?;
        record.tool_allowlist = allowlist
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .context("a stored tool allowlist")?;
        out.push(record);
    }
    let mut stmt = conn.prepare(SELECT_MOUNTS)?;
    let mounts = stmt.query_map(params![owner, id], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
    for mount in mounts {
        let (connection, host) = mount?;
        if let Some(record) = out.iter_mut().find(|c| c.id == connection) {
            record.mounts.push(host);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    /// Every (table, column) `sql` reads, as SQLite's authorizer reports.
    fn reads(conn: &Connection, sql: &str) -> BTreeSet<(String, String)> {
        let seen: Arc<Mutex<BTreeSet<(String, String)>>> = Arc::default();
        let sink = seen.clone();
        conn.authorizer(Some(move |ctx: AuthContext<'_>| {
            if let AuthAction::Read {
                table_name,
                column_name,
            } = ctx.action
            {
                sink.lock()
                    .unwrap()
                    .insert((table_name.to_string(), column_name.to_string()));
            }
            Authorization::Allow
        }))
        .unwrap();
        conn.prepare(sql).unwrap();
        conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>).unwrap();
        Arc::try_unwrap(seen).unwrap().into_inner().unwrap()
    }

    /// Plan 8a decision 3 (gateway spec §2, "list endpoints never read
    /// those tables"): what lists and shows a connection reads of
    /// `gw_credentials` is whether a row is there, by its key and owner,
    /// and nothing of the secret.
    #[test]
    fn listing_connections_reads_no_secret() {
        let store = GatewayStore::open_in_memory().unwrap();
        let conn = store.conn();
        let mut credentials = BTreeSet::new();
        for sql in [SELECT_CONNECTIONS, SELECT_MOUNTS] {
            for (table, column) in reads(&conn, sql) {
                if table == "gw_credentials" {
                    credentials.insert(column);
                }
            }
        }
        let expected: BTreeSet<String> = ["connection_id", "owner_id"].map(String::from).into();
        assert_eq!(credentials, expected);
    }
}
```

Replace the whole of `crates/hennery-gateway/src/lib.rs` with:

```rust
//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod store;
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `nix develop -c cargo test -p hennery-gateway --locked` and `nix develop -c cargo test -p hennery-testkit --locked --test owner_filter`
Expected: PASS: `store` 15 tests, `owner` 1, `boundary` 1, the store's unit test 1, and the audit's 3 with the gateway's 23 statements.

- [ ] **Step 5: Revert-probes**

| Line | Change | Fails |
|---|---|---|
| `update`'s credential `DELETE` | removed | `another_origin_or_kind_deletes_the_credential` |
| `update`'s status reset | removed | `another_origin_or_kind_deletes_the_credential` |
| `update`'s `url.origin() != old_origin`, then its `kind != current.cred_kind` | dropped, each | `another_origin_or_kind_deletes_the_credential` |
| `update`'s `Some(None) => None` (the allowlist cleared) | kept as stored | `an_update_changes_what_it_names_and_keeps_the_rest` |
| `delete`'s credential `DELETE`, then its mounts `DELETE` | removed, each | `a_deleted_connection_takes_its_mounts_and_credential_with_it` |
| `replace_mounts`'s live-host check | skipped | `mounts_are_replaced_as_a_set_of_the_owners_live_hosts` |
| its `revoked_at IS NULL` | dropped | `mounts_are_replaced_as_a_set_of_the_owners_live_hosts` |
| its `DELETE` before the inserts | removed | `mounts_are_replaced_as_a_set_of_the_owners_live_hosts` |
| `SELECT_MOUNTS`'s `h.revoked_at IS NULL` | dropped | `mounts_are_replaced_as_a_set_of_the_owners_live_hosts` |
| `create`'s hat check | skipped | `another_owners_connections_are_invisible_to_the_store` |
| `create`'s slug check | skipped | `a_slug_is_unique_per_owner_and_a_hat_must_be_the_owners` |
| `create`'s cap | skipped | `there_are_at_most_so_many_connections` |
| `create`'s, then `update`'s `is_supported` | skipped, each | `oauth_kinds_wait_for_plan_8f` |
| `set_static_credential`'s kind check, then its token check | skipped, each | `a_credential_goes_only_to_a_static_connection_and_is_checked` |
| `set_static_credential`'s `seal` | the token as it is | `a_static_credential_is_stored_sealed_and_opens_again` |
| `check_key`'s `open` | its error ignored | `a_static_credential_is_stored_sealed_and_opens_again` |
| `purge_hat`'s three `DELETE`s | removed, each | `purging_a_hat_deletes_its_connections_and_repeats_harmlessly` |
| `replace_mounts`' count cap, then its id length | skipped, each | `mounts_are_replaced_as_a_set_of_the_owners_live_hosts` |
| `static_credential`'s `url` | empty | `a_static_credential_is_stored_sealed_and_opens_again` |
| `ConnectionRecord`'s `Debug` of `url` | the whole URL | `a_connections_debug_shows_only_its_urls_origin` |
| `delete`'s `owner_id = ?2` | `?2 = ?2` | `another_owners_connections_are_invisible_to_the_store`; the audit's `every_query_of_the_stores_filters_by_the_owner` |
| `GATEWAY_STATEMENTS` | 24 | `every_query_of_the_stores_filters_by_the_owner` |
| `parse_url`'s `"http" if internal_network` | `"http"` | `what_a_connection_is_made_of_is_checked`, `http_stays_only_while_the_connection_is_internal`, `another_origin_or_kind_deletes_the_credential` |
| `parse_url`'s user name and password check | skipped | `what_a_connection_is_made_of_is_checked` |
| `header_problem`'s reserved names | skipped | `what_a_connection_is_made_of_is_checked` |
| `hennery-sessions` added to the gateway's `[dev-dependencies]` | | `the_gateway_never_depends_on_the_sessions_module` |

- [ ] **Step 6: The five checks, then commit**

```bash
git add Cargo.lock crates/hennery-gateway crates/hennery-testkit
git diff --cached --stat
git -c commit.gpgsign=false commit -m "feat(gateway): connections, mounts and the static credential in the store"
```

(The tests first, as `test(gateway): connections, mounts and the static credential in the store`.)

---

### Task 3: The connections API, behind step-up

**Files:**
- Modify: `crates/hennery-gateway/Cargo.toml`, `src/lib.rs`, `Cargo.lock`, `crates/hennery-proto/Cargo.toml`, `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Create: `crates/hennery-gateway/src/api.rs`
- Test: `crates/hennery-gateway/tests/api.rs`, `tests/api_log.rs`

**Interfaces:**
- Consumes: Task 2's `GatewayStore` and model; `hennery_kernel::auth::{operator_only, require_step_up, step_up_required}`, `hennery_kernel::json::ApiJson`, `hennery_kernel::operator::{Operator, Authenticated}`.
- Produces: `hennery_gateway::api::{GatewayState { store: Arc<GatewayStore>, key: Arc<MasterKey>, operator: Arc<Operator> }, router(GatewayState) -> Router}`; the wire types in `hennery_proto::rest`.

- [ ] **Step 1: Write the failing tests**

Replace the whole of `crates/hennery-gateway/Cargo.toml` with:

```toml
[package]
name = "hennery-gateway"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish.workspace = true

# Never `hennery-sessions`, in any table (umbrella §9, gateway spec §10):
# `tests/boundary.rs` reads the manifests and fails if it appears.
[dependencies]
anyhow.workspace = true
axum.workspace = true
chacha20poly1305.workspace = true
hennery-kernel.workspace = true
hennery-proto.workspace = true
hex.workspace = true
libc = "0.2"
rusqlite.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
url.workspace = true
zeroize.workspace = true

[dev-dependencies]
ed25519-dalek.workspace = true
# The store's unit tests read what its statements touch (`hooks`).
rusqlite = { workspace = true, features = ["hooks"] }
tempfile.workspace = true
tokio.workspace = true
toml.workspace = true
tower = { version = "0.5", features = ["util"] }
tracing-subscriber.workspace = true
```

Create `crates/hennery-gateway/tests/api.rs`:

```rust
//! The connections API (gateway spec §9, kernel spec §3.4): every route is
//! the operator's, behind the browser rules; creating, deleting, setting a
//! credential and changing where a token goes need a fresh step-up.
//! Driven through the router in-process.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::key::MasterKey;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::{Enrollment, Hosts};
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";
const PASSWORD: &str = "correct horse battery";

struct Api {
    _dir: tempfile::TempDir,
    app: Router,
    operator: Arc<Operator>,
    store: Arc<GatewayStore>,
    key: Arc<MasterKey>,
    hosts: Hosts,
    phc: String,
}

impl Api {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("hennery.db");
        let operator = Arc::new(Operator::open(&db).unwrap());
        let now = unix_now();
        let setup = operator.issue_setup_token(now).unwrap().unwrap();
        let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, PASSWORD, ORIGIN, now).unwrap() else {
            panic!("setup failed");
        };
        let store = Arc::new(GatewayStore::open(&db).unwrap());
        let key = Arc::new(MasterKey::from_bytes([5; 32]));
        let app = router(GatewayState {
            store: store.clone(),
            key: key.clone(),
            operator: operator.clone(),
        });
        Self {
            _dir: dir,
            app,
            operator,
            store,
            key,
            hosts: Hosts::open(&db).unwrap(),
            phc,
        }
    }

    /// A session whose last password check was `age` seconds ago.
    fn session(&self, age: i64) -> String {
        self.operator
            .open_session("test", &self.phc, unix_now() - age)
            .unwrap()
            .unwrap()
    }

    fn hat(&self) -> String {
        self.hosts.default_hat_for_new_hosts().unwrap()
    }

    fn host(&self, id: &str, seed: u8) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let enrollment = Enrollment {
            public_key: hex::encode(key.verifying_key().as_bytes()),
            name: format!("host {seed}"),
            host_version: "0.0.0".into(),
            platform: "macos-aarch64".into(),
        };
        self.hosts.register(id, &enrollment, unix_now()).unwrap();
    }

    async fn send(&self, session: &str, method: &str, path: &str, body: Option<&Value>) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"));
        let body = match body {
            Some(body) => {
                req = req.header("content-type", "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let resp = self.app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn new_body(&self, slug: &str) -> Value {
        json!({
            "slug": slug,
            "label": "Linear",
            "url": "https://mcp.linear.example/mcp",
            "hat_id": self.hat(),
            "cred_kind": "static",
        })
    }

    async fn create(&self, slug: &str) -> String {
        let (status, item) = self
            .send(
                &self.session(0),
                "POST",
                "/api/mcp/connections",
                Some(&self.new_body(slug)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{item}");
        item["id"].as_str().unwrap().to_string()
    }
}

fn code(body: &Value) -> &str {
    body["code"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_connection_is_created_listed_changed_and_deleted() {
    let api = Api::new();
    let s = api.session(0);
    let (status, item) = api
        .send(&s, "POST", "/api/mcp/connections", Some(&api.new_body("linear")))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = item["id"].as_str().unwrap().to_string();
    assert_eq!(item["slug"], "linear");
    assert_eq!(item["cred_kind"], "static");
    assert_eq!(item["static_header"], "Authorization");
    assert_eq!(item["static_prefix"], "Bearer ");
    assert_eq!(item["tool_allowlist"], Value::Null);
    assert_eq!(item["status"], "not_connected");
    assert_eq!(item["has_credential"], false);
    assert_eq!(item["mounts"], json!([]));
    assert!(item["created_at"].as_str().unwrap().ends_with('Z'), "{item}");
    assert!(
        item.get("status_note").is_none() && item.get("account_label").is_none(),
        "{item}"
    );

    let (status, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([item]));

    let path = format!("/api/mcp/connections/{id}");
    let (status, changed) = api
        .send(
            &s,
            "PATCH",
            &path,
            Some(&json!({ "label": "Linear (work)", "tool_allowlist": ["search"] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["label"], "Linear (work)");
    assert_eq!(changed["tool_allowlist"], json!(["search"]));
    // Absent keeps; `null` clears.
    let (_, kept) = api.send(&s, "PATCH", &path, Some(&json!({}))).await;
    assert_eq!(kept["tool_allowlist"], json!(["search"]));
    let (_, cleared) = api
        .send(&s, "PATCH", &path, Some(&json!({ "tool_allowlist": null })))
        .await;
    assert_eq!(cleared["tool_allowlist"], Value::Null);

    let (status, body) = api.send(&s, "DELETE", &path, None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    for (method, path, body) in [
        ("PATCH", path.clone(), Some(json!({ "label": "x" }))),
        ("DELETE", path.clone(), None),
        ("PUT", format!("{path}/mounts"), Some(json!({ "host_ids": [] }))),
        ("PUT", format!("{path}/credential"), Some(json!({ "token": "t" }))),
    ] {
        let (status, body) = api.send(&s, method, &path, body.as_ref()).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::NOT_FOUND, "not_found"),
            "{method} {path}"
        );
    }
}

/// Kernel spec §3.4, gateway spec §9, plan 8a decision 4: creating a
/// connection, deleting one, setting its credential and changing where its
/// token goes (url, kind, internal network, header, prefix) need a check
/// within five minutes; nothing is written without one. Its label, its
/// allowlist, its mounts and reading the list do not.
#[tokio::test]
async fn where_a_token_can_go_changes_only_with_a_fresh_step_up() {
    let api = Api::new();
    api.host("host-a", 1);
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}");
    let stale = api.session(5 * 60);
    let before = api.store.connection(&id).unwrap();
    let refused = [
        ("POST", "/api/mcp/connections".to_string(), Some(api.new_body("other"))),
        ("DELETE", path.clone(), None),
        ("PUT", format!("{path}/credential"), Some(json!({ "token": "tok" }))),
        (
            "PATCH",
            path.clone(),
            Some(json!({ "url": "https://elsewhere.example/" })),
        ),
        ("PATCH", path.clone(), Some(json!({ "cred_kind": "none" }))),
        ("PATCH", path.clone(), Some(json!({ "internal_network": true }))),
        ("PATCH", path.clone(), Some(json!({ "static_header": "X-API-Key" }))),
        ("PATCH", path.clone(), Some(json!({ "static_prefix": "" }))),
        // A connection that is not there: refused before it is looked up.
        (
            "PATCH",
            "/api/mcp/connections/conn-0000000000000000".to_string(),
            Some(json!({ "url": "https://elsewhere.example/" })),
        ),
        // Named with the same value it has, still: the check is on what is
        // named, before anything is read.
        (
            "PATCH",
            path.clone(),
            Some(json!({ "label": "x", "url": "https://mcp.linear.example/mcp" })),
        ),
    ];
    for (method, path, body) in &refused {
        let (status, answer) = api.send(&stale, method, path, body.as_ref()).await;
        assert_eq!(
            (status, code(&answer)),
            (StatusCode::FORBIDDEN, "step_up_required"),
            "{method} {path} {body:?}"
        );
    }
    assert_eq!(api.store.connection(&id).unwrap(), before);
    assert_eq!(api.store.list().unwrap().len(), 1);
    assert!(!api.store.has_ciphertext().unwrap());

    let free = [
        ("GET", "/api/mcp/connections".to_string(), None),
        ("PATCH", path.clone(), Some(json!({ "label": "Renamed" }))),
        ("PATCH", path.clone(), Some(json!({ "tool_allowlist": ["search"] }))),
        ("PUT", format!("{path}/mounts"), Some(json!({ "host_ids": ["host-a"] }))),
    ];
    for (method, path, body) in &free {
        let (status, answer) = api.send(&stale, method, path, body.as_ref()).await;
        assert_eq!(status, StatusCode::OK, "{method} {path}: {answer}");
    }

    let fresh = api.session(0);
    let accepted = [
        (
            "PATCH",
            path.clone(),
            Some(json!({ "static_header": "X-API-Key" })),
            StatusCode::OK,
        ),
        (
            "PUT",
            format!("{path}/credential"),
            Some(json!({ "token": "tok" })),
            StatusCode::NO_CONTENT,
        ),
        (
            "POST",
            "/api/mcp/connections".to_string(),
            Some(api.new_body("other")),
            StatusCode::CREATED,
        ),
        ("DELETE", path.clone(), None, StatusCode::NO_CONTENT),
    ];
    for (method, path, body, expected) in &accepted {
        let (status, answer) = api.send(&fresh, method, path, body.as_ref()).await;
        assert_eq!(status, *expected, "{method} {path}: {answer}");
    }
}

#[tokio::test]
async fn every_route_needs_the_operators_session_and_origin() {
    let api = Api::new();
    let id = api.create("linear").await;
    for (method, path) in [
        ("GET", "/api/mcp/connections".to_string()),
        ("POST", "/api/mcp/connections".to_string()),
        ("PATCH", format!("/api/mcp/connections/{id}")),
        ("DELETE", format!("/api/mcp/connections/{id}")),
        ("PUT", format!("/api/mcp/connections/{id}/mounts")),
        ("PUT", format!("/api/mcp/connections/{id}/credential")),
    ] {
        let (status, body) = api.send("not-a-session", method, &path, Some(&json!({}))).await;
        assert_eq!(
            (status, code(&body)),
            (StatusCode::UNAUTHORIZED, "unauthenticated"),
            "{method} {path}"
        );
        if method != "GET" {
            let req = Request::builder()
                .method(method)
                .uri(&path)
                .header("origin", "https://evil.example")
                .header("cookie", format!("hennery_session={}", api.session(0)))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap();
            let resp = api.app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{method} {path}");
        }
    }
    assert_eq!(api.store.list().unwrap().len(), 1);
}

#[tokio::test]
async fn refusals_answer_with_their_code_and_write_nothing() {
    let api = Api::new();
    let s = api.session(0);
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}");
    let mut none = api.new_body("public");
    none["cred_kind"] = json!("none");
    let (_, public) = api.send(&s, "POST", "/api/mcp/connections", Some(&none)).await;
    let public = public["id"].as_str().unwrap().to_string();
    let mut oauth = api.new_body("oauth");
    oauth["cred_kind"] = json!("oauth_dcr");
    let mut bad_slug = api.new_body("Bad Slug");
    bad_slug["slug"] = json!("Bad Slug");
    let mut http = api.new_body("lan");
    http["url"] = json!("http://mcp.example/");
    let mut extra = api.new_body("extra");
    extra["owner_id"] = json!("owner-x");
    let cases = [
        (
            "POST",
            "/api/mcp/connections".to_string(),
            json!(api.new_body("linear")),
            409,
            "slug_taken",
        ),
        (
            "POST",
            "/api/mcp/connections".to_string(),
            oauth,
            400,
            "unsupported_cred_kind",
        ),
        ("POST", "/api/mcp/connections".to_string(), bad_slug, 400, "invalid"),
        ("POST", "/api/mcp/connections".to_string(), http, 400, "invalid"),
        ("POST", "/api/mcp/connections".to_string(), extra, 422, "invalid_body"),
        (
            "PATCH",
            path.clone(),
            json!({ "cred_kind": "oauth_client" }),
            400,
            "unsupported_cred_kind",
        ),
        // The slug and the hat cannot change: naming them is refused.
        ("PATCH", path.clone(), json!({ "slug": "renamed" }), 422, "invalid_body"),
        (
            "PATCH",
            path.clone(),
            json!({ "hat_id": api.hat() }),
            422,
            "invalid_body",
        ),
        ("PATCH", path.clone(), json!({ "label": "" }), 400, "invalid"),
        (
            "PUT",
            format!("{path}/mounts"),
            json!({ "host_ids": ["host-x"] }),
            400,
            "invalid",
        ),
        (
            "PUT",
            format!("{path}/credential"),
            json!({ "token": "two words" }),
            400,
            "invalid",
        ),
        (
            "PUT",
            format!("/api/mcp/connections/{public}/credential"),
            json!({ "token": "t" }),
            409,
            "wrong_cred_kind",
        ),
    ];
    for (method, path, body, status, expected) in cases {
        let (got, answer) = api.send(&s, method, &path, Some(&body)).await;
        assert_eq!(
            (got.as_u16(), code(&answer)),
            (status, expected),
            "{method} {path} {body}"
        );
    }
    assert_eq!(api.store.list().unwrap().len(), 2);
    assert!(!api.store.has_ciphertext().unwrap());
    // A body past the limit is refused before it is read.
    let huge = json!({ "token": "x".repeat(300 * 1024) });
    let (status, answer) = api.send(&s, "PUT", &format!("{path}/credential"), Some(&huge)).await;
    assert_eq!(
        (status, code(&answer)),
        (StatusCode::PAYLOAD_TOO_LARGE, "body_too_large")
    );
}

#[tokio::test]
async fn a_static_credential_is_write_only() {
    let api = Api::new();
    let s = api.session(0);
    let id = api.create("linear").await;
    let (status, body) = api
        .send(
            &s,
            "PUT",
            &format!("/api/mcp/connections/{id}/credential"),
            Some(&json!({ "token": "the-token" })),
        )
        .await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    assert_eq!(
        api.store
            .static_credential(&id, &api.key)
            .unwrap()
            .unwrap()
            .token
            .as_str(),
        "the-token"
    );
    let (_, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(list[0]["has_credential"], true);
    assert!(!list.to_string().contains("the-token"), "{list}");
    // Another origin deletes it (gateway spec §4.6).
    let (_, moved) = api
        .send(
            &s,
            "PATCH",
            &format!("/api/mcp/connections/{id}"),
            Some(&json!({ "url": "https://elsewhere.example/mcp" })),
        )
        .await;
    assert_eq!(moved["has_credential"], false);
    assert_eq!(api.store.static_credential(&id, &api.key).unwrap(), None);
}

#[tokio::test]
async fn mounts_are_replaced_and_revoked_hosts_left_out() {
    let api = Api::new();
    api.host("host-a", 1);
    api.host("host-b", 2);
    let s = api.session(0);
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}/mounts");
    let (status, item) = api
        .send(&s, "PUT", &path, Some(&json!({ "host_ids": ["host-b", "host-a"] })))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(item["mounts"], json!(["host-a", "host-b"]));
    api.hosts.revoke("host-b", unix_now()).unwrap();
    let (_, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(list[0]["mounts"], json!(["host-a"]));
    let (status, answer) = api
        .send(&s, "PUT", &path, Some(&json!({ "host_ids": ["host-b"] })))
        .await;
    assert_eq!((status, code(&answer)), (StatusCode::BAD_REQUEST, "invalid"));
}

/// The review's O6: no answer of the gateway's is kept by a cache, the
/// list and the refusals alike.
#[tokio::test]
async fn answers_are_never_cached() {
    let api = Api::new();
    let s = api.session(0);
    for path in [
        "/api/mcp/connections",
        "/api/mcp/connections/conn-0000000000000000/mounts",
    ] {
        let method = if path.ends_with("mounts") { "PUT" } else { "GET" };
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={s}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"host_ids":[]}"#))
            .unwrap();
        let resp = api.app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.headers().get("cache-control").map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{method} {path}"
        );
    }
    // The refusals of the session and the browser rules too (the
    // re-confirmation's finding 4): no session (401), another origin (403).
    for (origin, cookie, status) in [
        (ORIGIN, None, StatusCode::UNAUTHORIZED),
        ("https://elsewhere.example", Some(&s), StatusCode::FORBIDDEN),
    ] {
        let mut req = Request::builder()
            .method("GET")
            .uri("/api/mcp/connections")
            .header("origin", origin);
        if let Some(cookie) = cookie {
            req = req.header("cookie", format!("hennery_session={cookie}"));
        }
        let resp = api.app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), status, "{origin}");
        assert_eq!(
            resp.headers().get("cache-control").map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "{status}"
        );
    }
}

/// The operator's decision of 2026-10-02 (through the gateway lane): a
/// connection marked `internal_network` may use plain `http`, set on
/// create or by one `PATCH` naming both, behind step-up; clearing the mark
/// while the URL is `http` is refused and changes nothing (plan 8a
/// decision 9).
#[tokio::test]
async fn an_internal_connection_may_use_http_and_keeps_its_mark_while_it_does() {
    let api = Api::new();
    let s = api.session(0);
    let id = api.create("lan").await;
    let path = format!("/api/mcp/connections/{id}");
    let lan = "http://10.0.0.7:8080/mcp";
    let (status, body) = api.send(&s, "PATCH", &path, Some(&json!({ "url": lan }))).await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"), "{body}");
    let stale = api.session(5 * 60);
    let both = json!({ "url": lan, "internal_network": true });
    let (status, body) = api.send(&stale, "PATCH", &path, Some(&both)).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "step_up_required"));
    let (status, item) = api.send(&s, "PATCH", &path, Some(&both)).await;
    assert_eq!(status, StatusCode::OK, "{item}");
    assert_eq!(
        (item["url"].as_str(), item["internal_network"].as_bool()),
        (Some(lan), Some(true))
    );
    let (status, body) = api
        .send(&s, "PATCH", &path, Some(&json!({ "internal_network": false })))
        .await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid"), "{body}");
    let (_, list) = api.send(&s, "GET", "/api/mcp/connections", None).await;
    assert_eq!(
        (list[0]["url"].as_str(), list[0]["internal_network"].as_bool()),
        (Some(lan), Some(true))
    );
    let mut marked = api.new_body("lan-2");
    marked["url"] = json!("http://192.168.1.9/mcp");
    marked["internal_network"] = json!(true);
    let (status, item) = api.send(&s, "POST", "/api/mcp/connections", Some(&marked)).await;
    assert_eq!(status, StatusCode::CREATED, "{item}");
}

/// The fleet parent's ruling (2026-10-02): a gateway error is the shared
/// `ApiError`, `{code, message}` and nothing else, over HTTP: a 404, a 400
/// and the step-up 403.
#[tokio::test]
async fn an_error_is_the_shared_api_error_and_nothing_more() {
    let api = Api::new();
    let id = api.create("linear").await;
    let path = format!("/api/mcp/connections/{id}");
    let fresh = api.session(0);
    let stale = api.session(5 * 60);
    for (session, path, body, status, expected) in [
        (
            &fresh,
            "/api/mcp/connections/conn-0000000000000000",
            json!({ "label": "Linear" }),
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            &fresh,
            path.as_str(),
            json!({ "label": "" }),
            StatusCode::BAD_REQUEST,
            "invalid",
        ),
        (
            &stale,
            path.as_str(),
            json!({ "url": "https://mcp.example/" }),
            StatusCode::FORBIDDEN,
            "step_up_required",
        ),
    ] {
        let (got, answer) = api.send(session, "PATCH", path, Some(&body)).await;
        assert_eq!(got, status, "{answer}");
        let mut keys: Vec<&str> = answer
            .as_object()
            .expect("a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "message"], "{answer}");
        assert_eq!(answer["code"], expected, "{answer}");
        assert!(answer["message"].as_str().is_some_and(|m| !m.is_empty()), "{answer}");
    }
}
```

Create `crates/hennery-gateway/tests/api_log.rs`:

```rust
//! Token hygiene (gateway spec §11, kernel spec §7, lane L8): a static
//! token sent to the API appears in no log line, at any level, in no
//! answer, and nowhere in `hennery.db` as written, whether the request is
//! taken or refused. In a test binary of its own, driven in-process on the
//! test's own task, so `tracing`'s thread-local subscriber sees every
//! event.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hennery_gateway::api::{GatewayState, router};
use hennery_gateway::key::MasterKey;
use hennery_gateway::store::GatewayStore;
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::{Operator, SetupOutcome};
use hennery_kernel::secret::unix_now;
use hennery_proto::rest::McpCredentialRequest;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const ORIGIN: &str = "https://hennery.example";
const SECRET: &str = "hnry-test-SECRET-0123456789abcdef";

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

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[tokio::test]
async fn a_static_token_is_never_logged_answered_or_stored_in_clear() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    let session = operator.open_session("test", &phc, now).unwrap().unwrap();
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let store = Arc::new(GatewayStore::open(&db).unwrap());
    let key = Arc::new(MasterKey::from_bytes([5; 32]));
    let app = router(GatewayState {
        store: store.clone(),
        key: key.clone(),
        operator,
    });
    let send = |method: &str, path: &str, body: String| {
        Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    };
    let mut answers = Vec::new();
    let mut call = async |method: &str, path: &str, body: serde_json::Value| {
        let resp = app.clone().oneshot(send(method, path, body.to_string())).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        answers.extend_from_slice(&bytes);
        (status, bytes)
    };

    let mut ids = Vec::new();
    for (slug, kind) in [("linear", "static"), ("public", "none")] {
        let (status, bytes) = call(
            "POST",
            "/api/mcp/connections",
            json!({ "slug": slug, "label": "L", "url": "https://mcp.example/", "hat_id": hat, "cred_kind": kind }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let item: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        ids.push(item["id"].as_str().unwrap().to_string());
    }
    let credential = |id: &str| format!("/api/mcp/connections/{id}/credential");
    // Refused: the wrong type, invalid characters, the wrong kind, an
    // unknown field beside it, an unknown connection.
    for (path, body, status) in [
        (
            credential(&ids[0]),
            json!({ "token": [SECRET] }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            credential(&ids[0]),
            json!({ "token": format!("{SECRET} x") }),
            StatusCode::BAD_REQUEST,
        ),
        (
            credential(&ids[0]),
            json!({ "token": format!("{SECRET}\n") }),
            StatusCode::BAD_REQUEST,
        ),
        (credential(&ids[1]), json!({ "token": SECRET }), StatusCode::CONFLICT),
        (
            credential(&ids[0]),
            json!({ "token": SECRET, "extra": SECRET }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            credential("conn-0000000000000000"),
            json!({ "token": SECRET }),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(call("PUT", &path, body.clone()).await.0, status, "{body}");
    }
    assert!(!store.has_ciphertext().unwrap());
    // Taken.
    let (status, _) = call("PUT", &credential(&ids[0]), json!({ "token": SECRET })).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        store.static_credential(&ids[0], &key).unwrap().unwrap().token.as_str(),
        SECRET
    );
    let (status, _) = call("GET", "/api/mcp/connections", json!(null)).await;
    assert_eq!(status, StatusCode::OK);

    let log = captured.0.lock().unwrap().clone();
    assert!(!log.is_empty(), "nothing was captured");
    assert!(contains(&log, &ids[0]), "the credential's connection is not logged");
    assert!(!contains(&log, SECRET), "{}", String::from_utf8_lossy(&log));
    assert!(!contains(&answers, SECRET), "{}", String::from_utf8_lossy(&answers));
    for file in ["hennery.db", "hennery.db-wal"] {
        let path = dir.path().join(file);
        if let Ok(bytes) = std::fs::read(&path) {
            assert!(!contains(&bytes, SECRET), "{file} holds the token in clear");
        }
    }
    let shown = format!("{:?}", McpCredentialRequest { token: SECRET.into() });
    assert!(!shown.contains(SECRET), "{shown}");
}

/// Plan 8a decision 19 (lane L11): some vendors put a secret in the URL's
/// path or query. A full upstream URL is in no log line and no error body,
/// and no `Debug` of a connection type shows more than its origin; the
/// list still answers the URL as stored, to its owner.
#[tokio::test]
async fn an_upstream_url_is_logged_and_shown_only_as_its_origin() {
    use hennery_proto::rest::{CreateMcpConnectionRequest, McpConnectionItem, McpCredKind, UpdateMcpConnectionRequest};
    const CANARY: &str = "c4n4ry-url-secret";
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    let now = unix_now();
    let setup = operator.issue_setup_token(now).unwrap().unwrap();
    let SetupOutcome::Done { phc, .. } = operator.set_up(&setup, "correct horse battery", ORIGIN, now).unwrap() else {
        panic!("setup failed");
    };
    let session = operator.open_session("test", &phc, now).unwrap().unwrap();
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let app = router(GatewayState {
        store: Arc::new(GatewayStore::open(&db).unwrap()),
        key: Arc::new(MasterKey::from_bytes([5; 32])),
        operator,
    });
    let url = format!("https://mcp.vendor.example:8443/s/{CANARY}/mcp?key={CANARY}");
    let mut refusals = Vec::new();
    let mut call = async |method: &str, path: &str, body: serde_json::Value| {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", ORIGIN)
            .header("cookie", format!("hennery_session={session}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        if !status.is_success() {
            refusals.extend_from_slice(&bytes);
        }
        (status, bytes)
    };
    let create = json!({ "slug": "vendor", "label": "V", "url": url, "hat_id": hat, "cred_kind": "static" });
    let (status, created) = call("POST", "/api/mcp/connections", create.clone()).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = serde_json::from_slice::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/mcp/connections/{id}");
    for (method, path, body, expected) in [
        (
            "POST",
            "/api/mcp/connections".to_string(),
            create.clone(),
            StatusCode::CONFLICT,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "label": "Vendor", "url": url }),
            StatusCode::OK,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "url": url.replace("https", "http") }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "label": "", "url": url }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "url": format!("{url}#{CANARY}") }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PUT",
            format!("{path}/mounts"),
            json!({ "host_ids": ["host-x"] }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "PATCH",
            path.clone(),
            json!({ "internal_network": url }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(call(method, &path, body.clone()).await.0, expected, "{method} {body}");
    }
    let (status, list) = call("GET", "/api/mcp/connections", json!(null)).await;
    assert_eq!(status, StatusCode::OK);
    let items: Vec<McpConnectionItem> = serde_json::from_slice(&list).unwrap();
    assert_eq!(items[0].url, url, "the owner's list keeps the URL as stored");

    let log = captured.0.lock().unwrap().clone();
    assert!(
        contains(&log, "https://mcp.vendor.example:8443"),
        "the origin is not logged"
    );
    assert!(!contains(&log, CANARY), "{}", String::from_utf8_lossy(&log));
    assert!(!refusals.is_empty());
    assert!(!contains(&refusals, CANARY), "{}", String::from_utf8_lossy(&refusals));
    let create: CreateMcpConnectionRequest = serde_json::from_value(create).unwrap();
    let update = UpdateMcpConnectionRequest {
        label: None,
        url: Some(url.clone()),
        cred_kind: Some(McpCredKind::Static),
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: None,
    };
    for shown in [format!("{:?}", items[0]), format!("{create:?}"), format!("{update:?}")] {
        assert!(!shown.contains(CANARY), "{shown}");
        assert!(shown.contains("https://mcp.vendor.example:8443"), "{shown}");
    }
    assert_eq!(
        hennery_proto::rest::url_origin("https://user:pw@h.example:1/x?y#z"),
        "https://h.example:1"
    );
    // Raw input, as a request holds it before any check (the
    // re-confirmation's finding 1): its `Debug` shows an origin or nothing.
    for raw in [
        format!("https://user:{CANARY}/x@h.example/"),
        format!("{CANARY} https://h.example/p"),
        format!("https://h.example\\{CANARY}"),
        format!("https://{CANARY}@h.example/"),
        format!("https://h.example/{CANARY}%2F@x"),
        format!("mailto:{CANARY}@h.example"),
        CANARY.to_string(),
    ] {
        let create = CreateMcpConnectionRequest {
            url: raw.clone(),
            ..create.clone()
        };
        let update = UpdateMcpConnectionRequest {
            url: Some(raw.clone()),
            ..update.clone()
        };
        for shown in [
            format!("{create:?}"),
            format!("{update:?}"),
            hennery_proto::rest::url_origin(&raw),
        ] {
            assert!(!shown.contains(CANARY), "{raw}: {shown}");
        }
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo build --workspace` (no `--locked`: it records the gateway's `hennery-proto` and test dependencies in `Cargo.lock`)
Then: `nix develop -c cargo test -p hennery-gateway --locked --test api --test api_log`
Expected: FAIL to compile: `unresolved import hennery_gateway::api`, `hennery_proto::rest::McpCredentialRequest`.

- [ ] **Step 3: Write the implementation**

Append to `crates/hennery-proto/src/rest.rs`:

```rust
/// How an MCP gateway connection authenticates to its upstream (gateway
/// spec §2). Plan 8a takes `none` and `static`; the OAuth kinds come with
/// plan 8f.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpCredKind {
    None,
    Static,
    OauthDcr,
    OauthClient,
}

/// A connection's health (gateway spec §7). Plan 8a only ever reports
/// `not_connected`; the probe (plan 8f) sets the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionStatus {
    NotConnected,
    Ok,
    NeedsAuth,
    Error,
}

/// One MCP gateway connection (gateway spec §2, §9): an entry of
/// `GET /api/mcp/connections`, and the answer to its creation and changes.
/// Never its secret: `has_credential` says whether one is stored.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpConnectionItem {
    pub id: String,
    /// `^[a-z0-9][a-z0-9-]{0,47}$`, unique per owner; fixed once created.
    pub slug: String,
    pub label: String,
    pub url: String,
    /// Fixed once created: a grant stays in its hat.
    pub hat_id: String,
    pub cred_kind: McpCredKind,
    /// The header a static token is sent in, and what goes before it.
    pub static_header: String,
    pub static_prefix: String,
    /// `null`: every tool.
    #[ts(type = "string[] | null")]
    pub tool_allowlist: Option<Vec<String>>,
    pub internal_network: bool,
    pub status: McpConnectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub status_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub account_label: Option<String>,
    /// RFC 3339, as the other stamps.
    pub status_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub has_credential: bool,
    /// The hosts it is mounted on, by id; revoked hosts are left out.
    pub mounts: Vec<String>,
}

/// `POST /api/mcp/connections` (step-up). Absent: the `Authorization`
/// header with `Bearer `, every tool, a public upstream.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateMcpConnectionRequest {
    pub slug: String,
    pub label: String,
    pub url: String,
    pub hat_id: String,
    pub cred_kind: McpCredKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string[] | null | undefined", optional)]
    pub tool_allowlist: Option<Vec<String>>,
    #[serde(default)]
    #[ts(type = "boolean | undefined", optional)]
    pub internal_network: bool,
}

/// `PATCH /api/mcp/connections/{id}`: an absent field keeps its value; a
/// `null` allowlist clears it (every tool), `""` clears the prefix (gateway
/// spec §4.6). Naming `url`, `cred_kind`, `internal_network`,
/// `static_header` or `static_prefix` needs step-up. Another origin or
/// kind deletes the stored credential. The slug and the hat cannot change.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateMcpConnectionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "McpCredKind | undefined", optional)]
    pub cred_kind: Option<McpCredKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_prefix: Option<String>,
    /// Absent: kept. `null`: cleared. A list: set.
    #[serde(default, with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<String>>")]
    #[ts(type = "string[] | null | undefined", optional)]
    pub tool_allowlist: Option<Option<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub internal_network: Option<bool>,
}

/// A field that may be absent, `null` or a value, kept apart: absent is
/// `None` (`#[serde(default)]`), `null` is `Some(None)`.
mod present {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_ref().and_then(Option::as_ref).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// `PUT /api/mcp/connections/{id}/mounts`: the whole set of hosts the
/// connection is mounted on, replacing the one before (gateway spec §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct McpMountsRequest {
    pub host_ids: Vec<String>,
}

/// `PUT /api/mcp/connections/{id}/credential` (step-up): a static token,
/// write-only (204). Its `Debug` never shows the token.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct McpCredentialRequest {
    pub token: String,
}

impl std::fmt::Debug for McpCredentialRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCredentialRequest")
            .field("token", &"<redacted>")
            .finish()
    }
}

/// What of an upstream URL a `Debug` may show (plan 8a decision 19):
/// `scheme://host[:port]`, without a user name, path, query or fragment,
/// any of which may hold a secret. Parsed as the gateway parses a URL
/// (`url::Url`), so raw input, before any check, fails closed: what does
/// not parse shows as `<not a url>`, a scheme without an origin as `null`
/// (the re-confirmation's finding 1).
pub fn url_origin(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(url) => url.origin().ascii_serialization(),
        Err(_) => "<not a url>".into(),
    }
}

impl std::fmt::Debug for McpConnectionItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpConnectionItem")
            .field("id", &self.id)
            .field("slug", &self.slug)
            .field("url", &url_origin(&self.url))
            .field("hat_id", &self.hat_id)
            .field("cred_kind", &self.cred_kind)
            .field("status", &self.status)
            .field("has_credential", &self.has_credential)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for CreateMcpConnectionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateMcpConnectionRequest")
            .field("slug", &self.slug)
            .field("url", &url_origin(&self.url))
            .field("hat_id", &self.hat_id)
            .field("cred_kind", &self.cred_kind)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for UpdateMcpConnectionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateMcpConnectionRequest")
            .field("label", &self.label)
            .field("url", &self.url.as_deref().map(url_origin))
            .field("cred_kind", &self.cred_kind)
            .field("internal_network", &self.internal_network)
            .finish_non_exhaustive()
    }
}
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SettingsUpdateRequest,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

with:

```rust
        rest::SettingsUpdateRequest,
        rest::McpCredKind,
        rest::McpConnectionStatus,
        rest::McpConnectionItem,
        rest::CreateMcpConnectionRequest,
        rest::UpdateMcpConnectionRequest,
        rest::McpMountsRequest,
        rest::McpCredentialRequest,
    );
    // `serde_json::Map` is a `BTreeMap` (always iterates sorted) by default,
```

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
        rest::SettingsUpdateRequest,
    );
    out
}
```

with:

```rust
        rest::SettingsUpdateRequest,
        rest::McpCredKind,
        rest::McpConnectionStatus,
        rest::McpConnectionItem,
        rest::CreateMcpConnectionRequest,
        rest::UpdateMcpConnectionRequest,
        rest::McpMountsRequest,
        rest::McpCredentialRequest,
    );
    out
}
```

In `crates/hennery-proto/Cargo.toml`, replace:

```toml
ts-rs.workspace = true
```

with:

```toml
ts-rs.workspace = true
# `url_origin`: an upstream URL in a `Debug`, parsed as the gateway parses it (plan 8a).
url.workspace = true
```

Create `crates/hennery-gateway/src/api.rs`:

```rust
//! The connections API (gateway spec §9): every route is the operator's
//! (`operator_only`: the browser rules, then the session cookie), and takes
//! its body as `ApiJson`. Creating a connection, deleting one, setting its
//! credential, and a change to where its token goes need a fresh step-up
//! (kernel spec §3.4; plan 8a decision 4). A token is never logged, never
//! answered, and stored only sealed.

use crate::key::MasterKey;
use crate::model::{
    Change, ConnectionPatch, ConnectionRecord, CredKind, CredentialChange, NewConnection, url_for_logs,
};
use crate::store::GatewayStore;
use axum::extract::{DefaultBodyLimit, Extension, Path, State};
use axum::handler::Handler;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Json, Router, middleware};
use hennery_kernel::auth::{operator_only, require_step_up, step_up_required};
use hennery_kernel::json::ApiJson;
use hennery_kernel::operator::{Authenticated, Operator};
use hennery_kernel::secret::{rfc3339, unix_now};
use hennery_proto::rest::{
    ApiError, CreateMcpConnectionRequest, McpConnectionItem, McpConnectionStatus, McpCredKind, McpCredentialRequest,
    McpMountsRequest, UpdateMcpConnectionRequest,
};
use std::sync::Arc;

/// The largest body a route reads: an allowlist of `MAX_TOOLS` names of
/// 128 bytes fits, a token of `MAX_TOKEN` many times over.
const BODY_LIMIT: usize = 256 * 1024;

#[derive(Clone)]
pub struct GatewayState {
    pub store: Arc<GatewayStore>,
    /// What credentials are sealed with (gateway spec §6).
    pub key: Arc<MasterKey>,
    /// The owner and their sessions (kernel spec §3).
    pub operator: Arc<Operator>,
}

/// The routes, behind the operator's session. Step-up is layered on each
/// method that always needs it (`Handler::layer`), so a method added to a
/// route later gets none unless it is layered too; `PATCH` checks it in
/// its handler, for the fields that need it.
pub fn router(state: GatewayState) -> Router {
    let routes = Router::new()
        .route(
            "/api/mcp/connections",
            get(list).post(create.layer(middleware::from_fn(require_step_up))),
        )
        .route(
            "/api/mcp/connections/{id}",
            patch(update).delete(delete.layer(middleware::from_fn(require_step_up))),
        )
        .route("/api/mcp/connections/{id}/mounts", put(mounts))
        .route(
            "/api/mcp/connections/{id}/credential",
            put(credential.layer(middleware::from_fn(require_step_up))),
        )
        .layer(DefaultBodyLimit::max(BODY_LIMIT));
    operator_only(routes, state.operator.clone())
        .layer(middleware::map_response(no_store))
        .with_state(state)
}

/// Every answer is private (connection URLs, hat and host ids), and none
/// is to be kept by a cache (the review's O6, as `projects.rs` does).
/// Outside `operator_only`, so its refusals carry it too (the
/// re-confirmation's finding 4).
async fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
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

fn internal(err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "internal error");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "no such connection")
}

fn wire_kind(kind: CredKind) -> McpCredKind {
    match kind {
        CredKind::None => McpCredKind::None,
        CredKind::Static => McpCredKind::Static,
        CredKind::OauthDcr => McpCredKind::OauthDcr,
        CredKind::OauthClient => McpCredKind::OauthClient,
    }
}

fn model_kind(kind: McpCredKind) -> CredKind {
    match kind {
        McpCredKind::None => CredKind::None,
        McpCredKind::Static => CredKind::Static,
        McpCredKind::OauthDcr => CredKind::OauthDcr,
        McpCredKind::OauthClient => CredKind::OauthClient,
    }
}

fn item(record: ConnectionRecord) -> McpConnectionItem {
    McpConnectionItem {
        status: match record.status.as_str() {
            "ok" => McpConnectionStatus::Ok,
            "needs_auth" => McpConnectionStatus::NeedsAuth,
            "error" => McpConnectionStatus::Error,
            _ => McpConnectionStatus::NotConnected,
        },
        id: record.id,
        slug: record.slug,
        label: record.label,
        url: record.url,
        hat_id: record.hat_id,
        cred_kind: wire_kind(record.cred_kind),
        static_header: record.static_header,
        static_prefix: record.static_prefix,
        tool_allowlist: record.tool_allowlist,
        internal_network: record.internal_network,
        status_note: record.status_note,
        account_label: record.account_label,
        status_at: rfc3339(record.status_at),
        created_at: rfc3339(record.created_at),
        updated_at: rfc3339(record.updated_at),
        has_credential: record.has_credential,
        mounts: record.mounts,
    }
}

fn changed(change: Change, status: StatusCode) -> Response {
    match change {
        Change::Done(record) => (status, Json(item(*record))).into_response(),
        Change::NotFound => not_found(),
        Change::SlugTaken => error(StatusCode::CONFLICT, "slug_taken", "another connection has this slug"),
        Change::TooMany => error(
            StatusCode::CONFLICT,
            "too_many_connections",
            format!("there are at most {} connections", crate::model::MAX_CONNECTIONS),
        ),
        Change::Unsupported(kind) => error(
            StatusCode::BAD_REQUEST,
            "unsupported_cred_kind",
            format!("{} connections are not supported yet", kind.as_str()),
        ),
        Change::Invalid(why) => error(StatusCode::BAD_REQUEST, "invalid", why),
    }
}

/// `GET /api/mcp/connections`: every connection, oldest first, without a
/// secret.
async fn list(State(state): State<GatewayState>) -> Response {
    match state.store.list() {
        Ok(records) => Json(records.into_iter().map(item).collect::<Vec<_>>()).into_response(),
        Err(err) => internal(err),
    }
}

/// `POST /api/mcp/connections` (step-up): 201 with the new connection.
async fn create(State(state): State<GatewayState>, ApiJson(req): ApiJson<CreateMcpConnectionRequest>) -> Response {
    let new = NewConnection {
        slug: req.slug,
        label: req.label,
        url: req.url,
        hat_id: req.hat_id,
        cred_kind: model_kind(req.cred_kind),
        static_header: req.static_header,
        static_prefix: req.static_prefix,
        tool_allowlist: req.tool_allowlist,
        internal_network: req.internal_network,
    };
    match state.store.create(&new, unix_now()) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(
                    connection_id = %record.id,
                    slug = %record.slug,
                    upstream = %url_for_logs(&record.url),
                    "gateway connection created"
                );
            }
            changed(change, StatusCode::CREATED)
        }
        Err(err) => internal(err),
    }
}

/// `PATCH /api/mcp/connections/{id}`: 200 with the connection as it is
/// now. Naming where its token goes (the URL, the kind, the internal
/// network, the header or the prefix) needs step-up, checked before
/// anything is read: those decide which upstream receives the credential
/// and how (plan 8a decision 4).
async fn update(
    State(state): State<GatewayState>,
    Extension(operator_session): Extension<Authenticated>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<UpdateMcpConnectionRequest>,
) -> Response {
    let moves_the_token = req.url.is_some()
        || req.cred_kind.is_some()
        || req.internal_network.is_some()
        || req.static_header.is_some()
        || req.static_prefix.is_some();
    if moves_the_token && !operator_session.stepped_up(unix_now()) {
        return step_up_required();
    }
    let patch = ConnectionPatch {
        label: req.label,
        url: req.url,
        cred_kind: req.cred_kind.map(model_kind),
        static_header: req.static_header,
        static_prefix: req.static_prefix,
        tool_allowlist: req.tool_allowlist,
        internal_network: req.internal_network,
    };
    match state.store.update(&id, &patch, unix_now()) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(
                    connection_id = %record.id,
                    upstream = %url_for_logs(&record.url),
                    has_credential = record.has_credential,
                    "gateway connection changed"
                );
            }
            changed(change, StatusCode::OK)
        }
        Err(err) => internal(err),
    }
}

/// `DELETE /api/mcp/connections/{id}` (step-up): 204, with its mounts and
/// credential gone.
async fn delete(State(state): State<GatewayState>, Path(id): Path<String>) -> Response {
    match state.store.delete(&id) {
        Ok(true) => {
            tracing::info!(connection_id = %id, "gateway connection deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => not_found(),
        Err(err) => internal(err),
    }
}

/// `PUT /api/mcp/connections/{id}/mounts`: 200 with the connection, its
/// mounts the given set.
async fn mounts(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<McpMountsRequest>,
) -> Response {
    match state.store.replace_mounts(&id, &req.host_ids) {
        Ok(change) => {
            if let Change::Done(record) = &change {
                tracing::info!(connection_id = %record.id, hosts = ?record.mounts, "gateway connection mounted");
            }
            changed(change, StatusCode::OK)
        }
        Err(err) => internal(err),
    }
}

/// `PUT /api/mcp/connections/{id}/credential` (step-up): 204, the static
/// token sealed and stored. Only the connection is logged.
async fn credential(
    State(state): State<GatewayState>,
    Path(id): Path<String>,
    ApiJson(req): ApiJson<McpCredentialRequest>,
) -> Response {
    match state
        .store
        .set_static_credential(&id, &req.token, &state.key, unix_now())
    {
        Ok(CredentialChange::Done) => {
            tracing::info!(connection_id = %id, "gateway connection's static credential set");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(CredentialChange::NotFound) => not_found(),
        Ok(CredentialChange::WrongKind(kind)) => error(
            StatusCode::CONFLICT,
            "wrong_cred_kind",
            format!("a {} connection takes no static token", kind.as_str()),
        ),
        Ok(CredentialChange::Invalid(why)) => error(StatusCode::BAD_REQUEST, "invalid", why),
        Err(err) => internal(err),
    }
}
```

Replace the whole of `crates/hennery-gateway/src/lib.rs` with:

```rust
//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod api;
pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod store;
```

Run: `nix develop -c cargo run -p hennery-proto --bin gen`
Expected: `wrote schema/hennery-protocol.schema.json` and `wrote web/src/generated/protocol.ts`. Without `--locked`, the run records `hennery-proto`'s `url` in `Cargo.lock`.

- [ ] **Step 4: Run the tests to see them pass**

Run: `nix develop -c cargo test -p hennery-gateway --locked`
Expected: PASS, `api` 9 tests and `api_log` 2, with Tasks 1 and 2's.

- [ ] **Step 5: Revert-probes**

| Line | Change | Fails |
|---|---|---|
| `POST`'s step-up layer | removed | `where_a_token_can_go_changes_only_with_a_fresh_step_up` |
| `DELETE`'s step-up layer | removed | the same |
| `PUT …/credential`'s step-up layer | removed | the same |
| `update`'s `stepped_up` check | skipped | the same |
| `moves_the_token`'s `url`, `cred_kind`, `internal_network`, `static_header`, `static_prefix` | dropped, each | the same |
| `DefaultBodyLimit::max(BODY_LIMIT)` | removed | `refusals_answer_with_their_code_and_write_nothing` |
| `deny_unknown_fields` on `UpdateMcpConnectionRequest`, then on `CreateMcpConnectionRequest` | dropped, each | `refusals_answer_with_their_code_and_write_nothing` |
| `tool_allowlist`'s `with = "present"` | dropped | `a_connection_is_created_listed_changed_and_deleted` |
| `McpCredentialRequest`'s `Debug` | shows the token | `a_static_token_is_never_logged_answered_or_stored_in_clear` |
| `McpConnectionItem`'s `Debug` of `url` | the whole URL | `an_upstream_url_is_logged_and_shown_only_as_its_origin` |
| the create's log line's `url_for_logs` | the whole URL | the same |
| `map_response(no_store)` | removed | `answers_are_never_cached` |
| `map_response(no_store)` outside `operator_only` | inside it | the same |
| `url_origin`'s `url::Url` parse | the split on `://` it replaced | `an_upstream_url_is_logged_and_shown_only_as_its_origin` |
| `error`'s `ApiError` | `session_id: Some("")`, then a `{error, message}` object | `an_error_is_the_shared_api_error_and_nothing_more` |
| `update`'s `ApiJson` | `axum::Json` | `an_upstream_url_is_logged_and_shown_only_as_its_origin` |

- [ ] **Step 6: The five checks, then commit**

```bash
git add Cargo.lock crates/hennery-gateway crates/hennery-proto schema web/src/generated
git diff --cached --stat
git -c commit.gpgsign=false commit -m "feat(gateway): the connections API, behind step-up"
```

(The tests first, as `test(gateway): the connections API, behind step-up, and token hygiene`.)

---

### Task 4: The collector opens the gateway and serves its routes

**Files:**
- Modify: `crates/hennery-gateway/src/lib.rs`, `crates/hennery/Cargo.toml`, `crates/hennery/src/main.rs`, `crates/hennery-host/src/adapter.rs`, `Cargo.lock`
- Test: `crates/hennery-gateway/tests/open.rs`, `crates/hennery/src/main.rs` (its tests), `crates/hennery/tests/cli.rs`

**Interfaces:**
- Consumes: `GatewayStore::{open, has_ciphertext, check_key}`, `key::{KeySource::from_env, load_or_create, KEY_ENV}`, `api::{GatewayState, router}`.
- Produces: `hennery_gateway::open(db: &Path, keys: &KeySource, operator: Arc<Operator>) -> anyhow::Result<GatewayState>`, and `hennery_gateway::KeyUnavailable`, the context of every error about the key (decision 8).

- [ ] **Step 1: Write the failing tests**

Create `crates/hennery-gateway/tests/open.rs`:

```rust
//! Opening the gateway on the collector's database (plan 8a decision 8): a
//! key that is missing while credentials are stored, or that does not open
//! them, is `KeyUnavailable`, told apart from a store that does not open,
//! so the collector could serve with the gateway off instead of refusing.

use hennery_gateway::key::{KEY_FILE, KeySource};
use hennery_gateway::model::{Change, CredKind, NewConnection};
use hennery_gateway::{KeyUnavailable, open};
use hennery_kernel::hosts::Hosts;
use hennery_kernel::operator::Operator;
use std::path::Path;
use std::sync::Arc;

fn keys(dir: &Path) -> KeySource {
    KeySource::from_vars(dir, None, None).unwrap()
}

#[test]
fn a_key_that_cannot_be_had_is_told_apart_from_a_store_that_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hennery.db");
    let operator = Arc::new(Operator::open(&db).unwrap());
    // The first start makes the key.
    let gateway = open(&db, &keys(dir.path()), operator.clone()).unwrap();
    assert!(dir.path().join(KEY_FILE).exists());
    let hat = Hosts::open(&db).unwrap().default_hat_for_new_hosts().unwrap();
    let new = NewConnection {
        slug: "linear".into(),
        label: "Linear".into(),
        url: "https://mcp.linear.example/mcp".into(),
        hat_id: hat,
        cred_kind: CredKind::Static,
        static_header: None,
        static_prefix: None,
        tool_allowlist: None,
        internal_network: false,
    };
    let Change::Done(record) = gateway.store.create(&new, 1).unwrap() else {
        panic!("not created");
    };
    gateway
        .store
        .set_static_credential(&record.id, "tok", &gateway.key, 1)
        .unwrap();
    drop(gateway);
    // Opened again: the same key opens the credential.
    open(&db, &keys(dir.path()), operator.clone()).unwrap();

    // Another key, then none.
    std::fs::write(dir.path().join(KEY_FILE), [9u8; 32]).unwrap();
    let err = open(&db, &keys(dir.path()), operator.clone()).err().unwrap();
    assert!(err.is::<KeyUnavailable>(), "{err:#}");
    std::fs::remove_file(dir.path().join(KEY_FILE)).unwrap();
    let err = open(&db, &keys(dir.path()), operator.clone()).err().unwrap();
    assert!(err.is::<KeyUnavailable>(), "{err:#}");
    assert!(format!("{err:#}").contains("is missing"), "{err:#}");
    assert!(!dir.path().join(KEY_FILE).exists());

    // A database that does not open is not about the key.
    let err = open(
        &dir.path().join("no-such-dir").join("hennery.db"),
        &keys(dir.path()),
        operator,
    )
    .err()
    .unwrap();
    assert!(!err.is::<KeyUnavailable>(), "{err:#}");
}
```

In `crates/hennery/Cargo.toml`, replace:

```toml
openssl = { workspace = true, optional = true }
hennery-host.workspace = true
```

with:

```toml
openssl = { workspace = true, optional = true }
hennery-gateway.workspace = true
hennery-host.workspace = true
```

In `crates/hennery/src/main.rs`, replace:

```rust
            .any(|(key, value)| key == "HENNERY_DEV_TOKEN" && value.is_none());
        assert!(removed, "the host child inherits HENNERY_DEV_TOKEN");
    }

    /// Decision 6: `up` hands its `--workspace-root` flags to its host
```

with:

```rust
            .any(|(key, value)| key == "HENNERY_DEV_TOKEN" && value.is_none());
        assert!(removed, "the host child inherits HENNERY_DEV_TOKEN");
    }

    /// Plan 8a decision 6: the master key may come from the environment
    /// (`HENNERY_MASTER_KEY`), which `up` passes to its collector; its host
    /// child, and so every agent, must not inherit it. The adapter spawn
    /// strips the same list (`HOST_SECRET_VARS`).
    #[test]
    fn ups_host_child_does_not_inherit_the_master_key() {
        assert!(hennery_host::adapter::HOST_SECRET_VARS.contains(&hennery_gateway::key::KEY_ENV));
        let args = UpArgs {
            listen: vec!["127.0.0.1:7117".into()],
            public_url: None,
            data_dir: "/nonexistent".into(),
            agents: Vec::new(),
            idle_timeout_secs: 0,
            workspace_roots: Vec::new(),
        };
        let cmd = host_command(
            std::path::Path::new("/bin/hennery"),
            std::path::Path::new("/nonexistent/host"),
            "ws://127.0.0.1:7117/api/hosts/ws",
            &args,
        );
        let removed = cmd
            .as_std()
            .get_envs()
            .any(|(key, value)| key == hennery_gateway::key::KEY_ENV && value.is_none());
        assert!(removed, "the host child inherits HENNERY_MASTER_KEY");
    }

    /// Decision 6: `up` hands its `--workspace-root` flags to its host
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
fn offline(cmd: &mut Command) {
    cmd.env("HENNERY_NPM_REGISTRY", OFFLINE)
        .env("HENNERY_NODE_MIRROR", OFFLINE);
    for var in LOG_VARS {
        cmd.env_remove(var);
    }
```

with:

```rust
fn offline(cmd: &mut Command) {
    cmd.env("HENNERY_NPM_REGISTRY", OFFLINE)
        .env("HENNERY_NODE_MIRROR", OFFLINE)
        // And never the gateway's key from whoever runs the tests (plan 8a):
        // a test that wants one sets it.
        .env_remove("HENNERY_MASTER_KEY")
        .env_remove("CREDENTIALS_DIRECTORY");
    for var in LOG_VARS {
        cmd.env_remove(var);
    }
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
    // removes the `-wal` and `-shm`.
    assert_eq!(mode_of(&dir.join("root")), 0o700);
    assert_eq!(mode_of(&data), 0o700);
    for file in [
        "hennery.db",
        "hennery.db-wal",
        "hennery.db-shm",
        "admin.sock",
        "vapid.key",
```

with:

```rust
    // removes the `-wal` and `-shm`.
    assert_eq!(mode_of(&dir.join("root")), 0o700);
    assert_eq!(mode_of(&data), 0o700);
    // The gateway's master key too (plan 8a).
    for file in [
        "hennery.db",
        "hennery.db-wal",
        "hennery.db-shm",
        "admin.sock",
        "vapid.key",
        "master.key",
```

In `crates/hennery/tests/cli.rs`, replace:

```rust
/// Review I1: `the_collectors_data_is_private_to_its_user` only sends its
```

with:

```rust
/// `method path` on the collector at `listen` with the owner's `session`,
/// from its `public_url`, with a JSON `body` if any: the status and the
/// body.
fn send_json(listen: &str, method: &str, path: &str, session: &str, body: Option<&str>) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(listen).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok()?;
    let body = body.unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {listen}\r\nCookie: hennery_session={session}\r\nOrigin: http://{listen}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    Some((head.split(' ').nth(1)?.parse().ok()?, body.to_string()))
}

/// Stop `collector` with SIGTERM, and wait for it.
fn stop(collector: &mut KillTree) {
    unsafe { libc::kill(collector.up.id() as i32, libc::SIGTERM) };
    assert!(wait_with_timeout(&mut collector.up, Duration::from_secs(15)).is_some());
}

/// Plan 8a (gateway spec §6, kernel spec §10): the collector makes its
/// master key at its first start and keeps it, serves the gateway's
/// connections, and a credential stored under it opens after a restart.
/// With the key gone while a credential is stored, the start fails, and no
/// new key is made.
#[test]
fn the_collector_keeps_its_master_key_and_will_not_start_without_it() {
    let dir = scratch_dir("master-key");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let (mut collector, listen) = collector_on(&data, &dir.join("first.log"));
    let session = sign_in(&mut collector, &listen, &data);
    let key = data.join("master.key");
    let made = std::fs::read(&key).unwrap();
    assert_eq!(made.len(), 32);
    assert_eq!(
        get_json(&listen, "/api/mcp/connections", &session),
        Some(serde_json::json!([]))
    );
    let hats = get_json(&listen, "/api/hats", &session).unwrap();
    let body = serde_json::json!({
        "slug": "linear",
        "label": "Linear",
        "url": "https://mcp.linear.example/mcp",
        "hat_id": hats[0]["id"],
        "cred_kind": "static",
    })
    .to_string();
    let (status, created) = send_json(&listen, "POST", "/api/mcp/connections", &session, Some(&body)).unwrap();
    assert_eq!(status, 201, "{created}");
    let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let credential = format!("/api/mcp/connections/{id}/credential");
    let (status, _) = send_json(&listen, "PUT", &credential, &session, Some(r#"{"token":"tok"}"#)).unwrap();
    assert_eq!(status, 204);
    let (status, _) = send_json(
        &listen,
        "PUT",
        "/api/mcp/connections/conn-0000000000000000/credential",
        &session,
        Some(r#"{"token":"tok"}"#),
    )
    .unwrap();
    assert_eq!(status, 404);
    stop(&mut collector);

    let (mut again, listen) = collector_on(&data, &dir.join("second.log"));
    assert_eq!(std::fs::read(&key).unwrap(), made, "the key was replaced");
    let listed = get_json(&listen, "/api/mcp/connections", &session).unwrap();
    assert_eq!(listed[0]["has_credential"], true, "{listed}");
    stop(&mut again);

    // Another key does not open the stored credential: refused.
    std::fs::write(&key, [9u8; 32]).unwrap();
    let stderr = refused_start(&data);
    assert!(stderr.contains("does not open the stored credential"), "{stderr}");
    // No key at all: refused, and none is made.
    std::fs::remove_file(&key).unwrap();
    let stderr = refused_start(&data);
    assert!(stderr.contains("master.key is missing"), "{stderr}");
    assert!(!key.exists(), "a new key was made");
}

/// Plan 8a decision 6 (the review's O4): a key from `HENNERY_MASTER_KEY`
/// is used and no `master.key` is made; started again without it once a
/// credential is stored, the collector refuses, and makes none either.
#[test]
fn a_master_key_from_the_environment_is_used_and_never_written_down() {
    let dir = scratch_dir("master-key-env");
    let _cleanup = RemoveDir(dir.clone());
    let data = dir.join("collector");
    let log = dir.join("collector.log");
    let child = hennery()
        .args(["collector", "--listen", "127.0.0.1:0"])
        .arg("--data-dir")
        .arg(&data)
        .env("HENNERY_MASTER_KEY", "ab".repeat(32))
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(log.with_extension("err")).unwrap())
        .spawn()
        .unwrap();
    let mut collector = KillTree::new(child, &log);
    let listen = collector.listening();
    let session = sign_in(&mut collector, &listen, &data);
    let hats = get_json(&listen, "/api/hats", &session).unwrap();
    let body = serde_json::json!({
        "slug": "linear",
        "label": "Linear",
        "url": "https://mcp.linear.example/mcp",
        "hat_id": hats[0]["id"],
        "cred_kind": "static",
    })
    .to_string();
    let (status, created) = send_json(&listen, "POST", "/api/mcp/connections", &session, Some(&body)).unwrap();
    assert_eq!(status, 201, "{created}");
    let id = serde_json::from_str::<serde_json::Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let credential = format!("/api/mcp/connections/{id}/credential");
    let (status, _) = send_json(&listen, "PUT", &credential, &session, Some(r#"{"token":"tok"}"#)).unwrap();
    assert_eq!(status, 204);
    stop(&mut collector);
    assert!(!data.join("master.key").exists(), "the key was written down");

    let stderr = refused_start(&data);
    assert!(stderr.contains("master.key is missing"), "{stderr}");
    assert!(!data.join("master.key").exists(), "a new key was made");
}

/// Start a collector on `data` that must fail to start: its standard error.
fn refused_start(data: &std::path::Path) -> String {
    let mut refused = hennery()
        .args(["collector", "--listen", "127.0.0.1:0", "--data-dir"])
        .arg(data)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let Some(status) = wait_with_timeout(&mut refused, Duration::from_secs(30)) else {
        let _ = refused.kill();
        panic!("the collector started");
    };
    let mut stderr = String::new();
    refused.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert!(!status.success(), "{stderr}");
    stderr
}

/// Review I1: `the_collectors_data_is_private_to_its_user` only sends its
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo build --workspace` (no `--locked`: it records the binary's new dependency in `Cargo.lock`)
Then: `nix develop -c cargo test -p hennery-gateway --locked --test open`
Expected: FAIL to compile: `unresolved imports hennery_gateway::KeyUnavailable, hennery_gateway::open`.
Then: `nix develop -c cargo test -p hennery --locked -- master_key collectors_data_is_private`
Expected: FAIL: `ups_host_child_does_not_inherit_the_master_key` (`HOST_SECRET_VARS` lacks it), `the_collectors_data_is_private_to_its_user` (`master.key: No such file or directory`), and the two `master_key` CLI tests (no `master.key`; the routes are not served).

- [ ] **Step 3: Write the implementation**

Replace the whole of `crates/hennery-gateway/src/lib.rs` with:

```rust
//! The MCP gateway (gateway spec): connections, their credentials at rest,
//! and the hosts they are mounted on. Depends on `hennery-kernel` and
//! `hennery-proto`, never on `hennery-sessions` (umbrella §9).

pub mod api;
pub mod crypto;
pub mod key;
pub mod model;
mod schema;
pub mod store;

use anyhow::Result;
use hennery_kernel::operator::Operator;
use std::path::Path;
use std::sync::Arc;

/// The master key could not be had: missing while credentials are stored,
/// unreadable, refused, or not the one that sealed them (plan 8a decision
/// 8). It is the context of every such error from `open`, so a caller can
/// tell it from a store that does not open. The collector refuses to start
/// on it today; serving with the gateway off (its routes 503, sessions
/// untouched) would match on it and serve a stand-in router instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the MCP gateway's master key is unavailable")]
pub struct KeyUnavailable;

/// The gateway on the collector's `hennery.db`: its store, and the master
/// key from `keys`, made at the first start. Before serving, and after the
/// admin socket's bind (the one-collector guard).
pub fn open(db: &Path, keys: &key::KeySource, operator: Arc<Operator>) -> Result<api::GatewayState> {
    let store = store::GatewayStore::open(db)?;
    let stored = store.has_ciphertext()?;
    let (key, origin) = key::load_or_create(keys, stored).map_err(|err| err.context(KeyUnavailable))?;
    store.check_key(&key).map_err(|err| err.context(KeyUnavailable))?;
    tracing::info!(source = ?origin, "the gateway's master key is loaded");
    Ok(api::GatewayState {
        store: Arc::new(store),
        key: Arc::new(key),
        operator,
    })
}
```

In `crates/hennery/src/main.rs`, replace:

```rust
    let mut state = AppState::new(store, hosts, operator);
    // Before anything serves: every push subscription is bound to this key
    // (kernel spec §6).
    state.vapid = std::sync::Arc::new(hennery_kernel::push::VapidKey::load_or_create(&args.data_dir)?);
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
```

with:

```rust
    let mut state = AppState::new(store, hosts, operator);
    // Before anything serves: every push subscription is bound to this key
    // (kernel spec §6).
    state.vapid = std::sync::Arc::new(hennery_kernel::push::VapidKey::load_or_create(&args.data_dir)?);
    // Before serving: the gateway's store, and the master key that opens
    // what it holds (plan 8a), from `HENNERY_MASTER_KEY`, a systemd
    // credential or `<data>/master.key`. A key that is missing while
    // credentials are stored, or that does not open them, stops the start
    // (plan 8a decision 8; `KeyUnavailable` tells that case apart).
    let keys = hennery_gateway::key::KeySource::from_env(&args.data_dir)?;
    let gateway = hennery_gateway::open(&db, &keys, state.operator.clone())?;
    state.offline_threshold = std::time::Duration::from_secs(args.host_offline_secs);
```

In `crates/hennery/src/main.rs`, replace:

```rust
        hennery_sessions::router(state.clone()),
```

with:

```rust
        hennery_sessions::router(state.clone()).merge(hennery_gateway::api::router(gateway)),
```

In `crates/hennery-host/src/adapter.rs`, replace:

```rust
/// script, say) are an agent's to read.
pub const HOST_SECRET_VARS: &[&str] = &["HENNERY_DEV_TOKEN"];
```

with:

```rust
/// script, say) are an agent's to read. `HENNERY_MASTER_KEY` opens every
/// credential the gateway holds (plan 8a): `up` passes it to its collector,
/// never to its host.
pub const HOST_SECRET_VARS: &[&str] = &["HENNERY_DEV_TOKEN", "HENNERY_MASTER_KEY"];
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `nix develop -c cargo test -p hennery-gateway --locked --test open` and `nix develop -c cargo test -p hennery --locked -- master_key collectors_data_is_private`
Expected: PASS: `open` 1 test; the unit test and the three CLI tests above.

- [ ] **Step 5: Revert-probes**

| Line | Change | Fails |
|---|---|---|
| the gateway's router merged | not merged | `the_collector_keeps_its_master_key_and_will_not_start_without_it` |
| `open`'s `store.has_ciphertext()?` | `false` | the same |
| `open`'s `store.check_key(&key)` | removed | the same |
| `open`'s `.context(KeyUnavailable)` on `check_key` | removed | `a_key_that_cannot_be_had_is_told_apart_from_a_store_that_does_not_open` |
| `HOST_SECRET_VARS`' `HENNERY_MASTER_KEY` | removed | `ups_host_child_does_not_inherit_the_master_key` |

`create_key_file`'s `.mode(0o600)` is probed again here by `the_collectors_data_is_private_to_its_user`, which runs the collector under `umask 022`.

- [ ] **Step 6: The five checks, then commit**

```bash
git add Cargo.lock crates/hennery-gateway crates/hennery crates/hennery-host
git diff --cached --stat
git -c commit.gpgsign=false commit -m "feat(gateway): the collector opens the gateway and its master key, and serves its routes"
```

(The tests first, as `test(gateway): the collector's master key and the gateway's routes`.)

---

### Task 5: The API, documented as a contract

**Files:**
- Modify: `crates/hennery-proto/src/rest.rs`, `crates/hennery-proto/src/codegen.rs`, `schema/hennery-protocol.schema.json`, `web/src/generated/protocol.ts`
- Test: `crates/hennery-proto/tests/codegen.rs`

**Interfaces:**
- Consumes: the seven wire types of Task 3, the codes of `api.rs`, `hennery_kernel::{auth, origin, json}`.
- Produces: no new type, field or code; the docs, and `render_ts` writing each type's `TS::docs()` above its declaration (decision 21).

- [ ] **Step 1: Write the failing tests**

Append to `crates/hennery-proto/tests/codegen.rs`:

```rust
/// The gateway's wire types (plan 8a), which the frontend builds on.
const GATEWAY_TYPES: [&str; 7] = [
    "McpCredKind",
    "McpConnectionStatus",
    "McpConnectionItem",
    "CreateMcpConnectionRequest",
    "UpdateMcpConnectionRequest",
    "McpMountsRequest",
    "McpCredentialRequest",
];

/// Plan 8a, Task 5: the gateway's API is a contract with the frontend, so
/// each of its wire types, each of their fields and each variant says what
/// it is. An enum whose variants have no doc is rendered as a bare `enum`
/// list, which fails here too.
#[test]
fn the_gateways_wire_types_document_every_field_and_variant() {
    let schema: serde_json::Value = serde_json::from_str(&render_schema()).unwrap();
    let described = |v: &serde_json::Value| v["description"].as_str().is_some_and(|d| !d.trim().is_empty());
    for name in GATEWAY_TYPES {
        let def = &schema["$defs"][name];
        assert!(described(def), "{name} has no doc: {def}");
        assert!(def.get("enum").is_none(), "{name}'s variants have no doc: {def}");
        for (field, prop) in def["properties"].as_object().into_iter().flatten() {
            assert!(described(prop), "{name}.{field} has no doc: {prop}");
        }
        for variant in def["oneOf"].as_array().into_iter().flatten() {
            assert!(described(variant), "{name}: a variant has no doc: {variant}");
        }
    }
}

/// Plan 8a, Task 5: a type's own doc (for the gateway's: its route, whether
/// it needs step-up, its answers and its error codes) reaches the
/// TypeScript, above its declaration, as it reaches the schema.
#[test]
fn a_types_doc_reaches_the_typescript() {
    let ts = render_ts();
    for name in GATEWAY_TYPES {
        let at = ts.find(&format!("\nexport type {name} =")).expect(name);
        assert!(ts[..at].ends_with("*/"), "{name}'s doc is not above its declaration");
    }
    // Every error code a gateway route answers, named in a doc.
    for code in [
        "unauthenticated",
        "setup_required",
        "origin_mismatch",
        "cross_site",
        "unsupported_media_type",
        "step_up_required",
        "invalid_body",
        "body_too_large",
        "not_found",
        "invalid",
        "unsupported_cred_kind",
        "slug_taken",
        "too_many_connections",
        "wrong_cred_kind",
        "internal",
    ] {
        assert!(ts.contains(&format!("`{code}`")), "no doc names `{code}`");
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `nix develop -c cargo test -p hennery-proto --locked --test codegen`
Expected: FAIL: `the_gateways_wire_types_document_every_field_and_variant` (`McpCredKind`'s variants have no doc) and `a_types_doc_reaches_the_typescript` (no type's doc is above its declaration).

- [ ] **Step 3: Write the implementation**

In `crates/hennery-proto/src/codegen.rs`, replace:

```rust
/// One TypeScript module exporting every hennery wire type.
pub fn render_ts() -> String {
    let cfg = Config::default();
    let mut out = String::from(HEADER);
    macro_rules! add {
        ($($t:ty),* $(,)?) => {$(
            out.push_str("\nexport ");
```

with:

```rust
/// One TypeScript module exporting every hennery wire type, each with its
/// own doc comment above it, as ts-rs's own export writes it (plan 8a: the
/// frontend reads a route's answers and error codes there). `decl` carries
/// only the fields' docs.
pub fn render_ts() -> String {
    let cfg = Config::default();
    let mut out = String::from(HEADER);
    macro_rules! add {
        ($($t:ty),* $(,)?) => {$(
            out.push('\n');
            out.push_str(&<$t as TS>::docs().unwrap_or_default());
            out.push_str("export ");
```

In `crates/hennery-proto/src/rest.rs`, replace:

```rust
/// How an MCP gateway connection authenticates to its upstream (gateway
/// spec §2). Plan 8a takes `none` and `static`; the OAuth kinds come with
/// plan 8f.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpCredKind {
    None,
    Static,
    OauthDcr,
    OauthClient,
}

/// A connection's health (gateway spec §7). Plan 8a only ever reports
/// `not_connected`; the probe (plan 8f) sets the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionStatus {
    NotConnected,
    Ok,
    NeedsAuth,
    Error,
}

/// One MCP gateway connection (gateway spec §2, §9): an entry of
/// `GET /api/mcp/connections`, and the answer to its creation and changes.
/// Never its secret: `has_credential` says whether one is stored.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpConnectionItem {
    pub id: String,
    /// `^[a-z0-9][a-z0-9-]{0,47}$`, unique per owner; fixed once created.
    pub slug: String,
    pub label: String,
    pub url: String,
    /// Fixed once created: a grant stays in its hat.
    pub hat_id: String,
    pub cred_kind: McpCredKind,
    /// The header a static token is sent in, and what goes before it.
    pub static_header: String,
    pub static_prefix: String,
    /// `null`: every tool.
    #[ts(type = "string[] | null")]
    pub tool_allowlist: Option<Vec<String>>,
    pub internal_network: bool,
    pub status: McpConnectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub status_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub account_label: Option<String>,
    /// RFC 3339, as the other stamps.
    pub status_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub has_credential: bool,
    /// The hosts it is mounted on, by id; revoked hosts are left out.
    pub mounts: Vec<String>,
}

/// `POST /api/mcp/connections` (step-up). Absent: the `Authorization`
/// header with `Bearer `, every tool, a public upstream.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateMcpConnectionRequest {
    pub slug: String,
    pub label: String,
    pub url: String,
    pub hat_id: String,
    pub cred_kind: McpCredKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string[] | null | undefined", optional)]
    pub tool_allowlist: Option<Vec<String>>,
    #[serde(default)]
    #[ts(type = "boolean | undefined", optional)]
    pub internal_network: bool,
}

/// `PATCH /api/mcp/connections/{id}`: an absent field keeps its value; a
/// `null` allowlist clears it (every tool), `""` clears the prefix (gateway
/// spec §4.6). Naming `url`, `cred_kind`, `internal_network`,
/// `static_header` or `static_prefix` needs step-up. Another origin or
/// kind deletes the stored credential. The slug and the hat cannot change.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateMcpConnectionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "McpCredKind | undefined", optional)]
    pub cred_kind: Option<McpCredKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_prefix: Option<String>,
    /// Absent: kept. `null`: cleared. A list: set.
    #[serde(default, with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<String>>")]
    #[ts(type = "string[] | null | undefined", optional)]
    pub tool_allowlist: Option<Option<Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub internal_network: Option<bool>,
}

/// A field that may be absent, `null` or a value, kept apart: absent is
/// `None` (`#[serde(default)]`), `null` is `Some(None)`.
mod present {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_ref().and_then(Option::as_ref).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// `PUT /api/mcp/connections/{id}/mounts`: the whole set of hosts the
/// connection is mounted on, replacing the one before (gateway spec §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct McpMountsRequest {
    pub host_ids: Vec<String>,
}

/// `PUT /api/mcp/connections/{id}/credential` (step-up): a static token,
/// write-only (204). Its `Debug` never shows the token.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct McpCredentialRequest {
    pub token: String,
```

with:

```rust
/// How an MCP gateway connection authenticates to its upstream (gateway
/// spec §2): `none`; `static`, a token the operator sets; `oauth_dcr` and
/// `oauth_client`, OAuth (plan 8f). Plan 8a takes `none` and `static`;
/// naming an OAuth kind in a create or a change answers 400
/// `unsupported_cred_kind` until plan 8f.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpCredKind {
    /// No credential: the gateway sends the agents' requests as they are.
    None,
    /// A token the operator sets (`PUT /api/mcp/connections/{id}/credential`),
    /// sent as `<static_header>: <static_prefix><token>`.
    Static,
    /// OAuth, the client registered dynamically (plan 8f).
    OauthDcr,
    /// OAuth, with a client the operator registered with the vendor (plan 8f).
    OauthClient,
}

/// A connection's health (gateway spec §7): `not_connected`, not checked
/// yet; `ok`; `needs_auth`, the operator must sign in again; `error`, with
/// `status_note`. Plan 8a only ever reports `not_connected`; the probe
/// (plan 8f) sets the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionStatus {
    /// Not checked yet: since it was created, or since another origin or
    /// kind deleted its credential.
    NotConnected,
    /// The last check reached the upstream with its credential.
    Ok,
    /// The upstream wants the operator to sign in again.
    NeedsAuth,
    /// The last check failed; `status_note` says how.
    Error,
}

/// One MCP gateway connection (gateway spec §2, §9): an entry of
/// `GET /api/mcp/connections` (200, an array, oldest first), and the answer
/// to `POST` (201), `PATCH` (200) and `PUT …/{id}/mounts` (200). Never its
/// secret: `has_credential` says whether one is stored.
///
/// Every `/api/mcp/*` route is the operator's and answers
/// `Cache-Control: no-store`. An error is an `ApiError`; the codes every
/// route may answer:
/// - 401 `unauthenticated`: no live session: sign in.
/// - 403 `setup_required`, `origin_mismatch` or `cross_site`: the browser
///   rules refused the request (kernel spec §3.2).
/// - 415 `unsupported_media_type`: a body that is not `application/json`.
/// - 400 `invalid_body`: a body that is not JSON; 422 `invalid_body`: one
///   that is not the route's shape, an unknown field included. The body is
///   never quoted back.
/// - 413 `body_too_large`: a body over 256 KiB.
/// - 405, with no body: a method the path does not take.
/// - 500 `internal`.
///
/// Each request type names the codes of its own route. A 403
/// `step_up_required` asks for a password or passkey check
/// (`POST /api/auth/step-up/…`); retry after it.
///
/// `DELETE /api/mcp/connections/{id}` (step-up) takes no body: 204, its
/// mounts and credential deleted with it; 403 `step_up_required`, 404
/// `not_found`.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct McpConnectionItem {
    /// `conn-` and 16 hexadecimal digits: the `{id}` of the routes.
    pub id: String,
    /// `^[a-z0-9][a-z0-9-]{0,47}$`, unique per owner; fixed once created.
    /// Agents see the server as `hennery-<slug>`.
    pub slug: String,
    /// The operator's name for it: 1 to 64 printable characters.
    pub label: String,
    /// The upstream MCP endpoint, as stored (parsed and serialised). Only
    /// this list shows it whole: logs and errors show only its origin.
    pub url: String,
    /// The hat whose sessions may use it. Fixed once created: a grant stays
    /// in its hat.
    pub hat_id: String,
    /// How it authenticates to its upstream.
    pub cred_kind: McpCredKind,
    /// The header a `static` token is sent in (by default `Authorization`).
    pub static_header: String,
    /// What goes before the token in that header (by default `Bearer `);
    /// may be empty.
    pub static_prefix: String,
    /// The tools an agent may call: `null`, every tool; `[]`, none; else
    /// these names, in order, without duplicates.
    #[ts(type = "string[] | null")]
    pub tool_allowlist: Option<Vec<String>>,
    /// The upstream is on the operator's own network: `http` is allowed,
    /// and private addresses are not refused (plan 8b).
    pub internal_network: bool,
    /// Its health.
    pub status: McpConnectionStatus,
    /// Why `status` is what it is, when the probe (plan 8f) says; absent
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub status_note: Option<String>,
    /// The upstream account it is signed in as, when the probe (plan 8f)
    /// learns it; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub account_label: Option<String>,
    /// When `status` last changed: RFC 3339, as the other stamps.
    pub status_at: String,
    /// When it was created: RFC 3339.
    pub created_at: String,
    /// When a `PATCH` last changed it: RFC 3339. Mounts and the credential
    /// do not move it.
    pub updated_at: String,
    /// Whether a credential is stored. No route answers the credential.
    pub has_credential: bool,
    /// The hosts it is mounted on, by id, sorted; revoked hosts are left out.
    pub mounts: Vec<String>,
}

/// `POST /api/mcp/connections` (step-up): 201 with the new
/// `McpConnectionItem`, `not_connected`, with no credential and no mounts.
/// Absent: the `Authorization` header with `Bearer `, every tool, a public
/// upstream. Unknown fields are refused.
///
/// Its own codes, beyond every route's (see `McpConnectionItem`): 403
/// `step_up_required`; 400 `invalid` (a field refused, `message` says
/// which, or a hat that is not the owner's); 400 `unsupported_cred_kind`
/// (an OAuth kind, until plan 8f); 409 `slug_taken` (another of the owner's
/// connections has the slug); 409 `too_many_connections` (at most 256 per
/// owner).
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct CreateMcpConnectionRequest {
    /// `^[a-z0-9][a-z0-9-]{0,47}$`, unique per owner; fixed once created.
    pub slug: String,
    /// 1 to 64 printable characters.
    pub label: String,
    /// Absolute `https`, or `http` only with `internal_network`; with a
    /// host, without a user name, password or fragment; at most 2048 bytes.
    /// A query is kept, but a secret belongs in the credential.
    pub url: String,
    /// One of the owner's hats; fixed once created.
    pub hat_id: String,
    /// `none` or `static` until plan 8f.
    pub cred_kind: McpCredKind,
    /// Absent: `Authorization`. An HTTP header name of at most 64 bytes,
    /// not one the gateway sets or filters itself (`host`, `content-type`,
    /// `cookie`, `mcp-session-id`, `proxy-*`, `sec-*` and the like).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_header: Option<String>,
    /// Absent: `Bearer `. At most 32 visible ASCII characters or spaces;
    /// `""` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_prefix: Option<String>,
    /// Absent or `null`: every tool. Else at most 1024 names, each 1 to 128
    /// visible ASCII characters; duplicates are dropped, the order kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string[] | null | undefined", optional)]
    pub tool_allowlist: Option<Vec<String>>,
    /// Absent: `false`. The upstream is on the operator's own network.
    #[serde(default)]
    #[ts(type = "boolean | undefined", optional)]
    pub internal_network: bool,
}

/// `PATCH /api/mcp/connections/{id}`: 200 with the `McpConnectionItem` as
/// it is now. An absent field keeps its value; a `null` allowlist clears it
/// (every tool), `""` clears the prefix (gateway spec §4.6); a `null` for
/// another field reads as absent. Naming `url`, `cred_kind`,
/// `internal_network`, `static_header` or `static_prefix` needs step-up,
/// even with its stored value. Another origin (scheme, host, port) or
/// another kind deletes the stored credential and starts the status over
/// at `not_connected`, in the same change. The slug and the hat cannot
/// change: naming them is an unknown field.
///
/// Its own codes, beyond every route's (see `McpConnectionItem`): 403
/// `step_up_required`; 404 `not_found`; 400 `invalid` (`message` says
/// which field); 400 `unsupported_cred_kind`. A refused change changes
/// nothing.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct UpdateMcpConnectionRequest {
    /// 1 to 64 printable characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub label: Option<String>,
    /// Step-up. As on create; another origin deletes the credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub url: Option<String>,
    /// Step-up. Another kind deletes the credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "McpCredKind | undefined", optional)]
    pub cred_kind: Option<McpCredKind>,
    /// Step-up. As on create; cannot be empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_header: Option<String>,
    /// Step-up. As on create; `""` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "string | undefined", optional)]
    pub static_prefix: Option<String>,
    /// Absent: kept. `null`: cleared (every tool). A list: set, as on
    /// create.
    #[serde(default, with = "present", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<String>>")]
    #[ts(type = "string[] | null | undefined", optional)]
    pub tool_allowlist: Option<Option<Vec<String>>>,
    /// Step-up. `false` is refused while the URL is `http`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "boolean | undefined", optional)]
    pub internal_network: Option<bool>,
}

/// A field that may be absent, `null` or a value, kept apart: absent is
/// `None` (`#[serde(default)]`), `null` is `Some(None)`.
mod present {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: Serialize>(value: &Option<Option<T>>, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_ref().and_then(Option::as_ref).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// `PUT /api/mcp/connections/{id}/mounts`: the whole set of hosts the
/// connection is mounted on, replacing the one before, never a delta
/// (gateway spec §9). No step-up: a mount reaches only a host the owner
/// paired. 200 with the `McpConnectionItem`.
///
/// Its own codes, beyond every route's (see `McpConnectionItem`): 404
/// `not_found`; 400 `invalid` (a host that is not one of the owner's
/// paired, unrevoked hosts; more than 1024 hosts; an id over 64 bytes,
/// which is not quoted back). A refused set changes nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct McpMountsRequest {
    /// Host ids; `[]` unmounts it everywhere. A repeated id counts once.
    pub host_ids: Vec<String>,
}

/// `PUT /api/mcp/connections/{id}/credential` (step-up): a static token,
/// write-only: 204, the token sealed and stored, replacing any before it.
/// No route reads it back or clears it; changing the kind or the origin,
/// or deleting the connection, deletes it. Its `Debug` never shows the
/// token.
///
/// Its own codes, beyond every route's (see `McpConnectionItem`): 403
/// `step_up_required`; 404 `not_found`; 409 `wrong_cred_kind` (the
/// connection is not `static`); 400 `invalid` (the token is not 1 to 8192
/// visible ASCII characters without spaces; never quoted).
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct McpCredentialRequest {
    /// The token, as the upstream takes it after `static_prefix`.
    pub token: String,
```

Run: `nix develop -c cargo run -p hennery-proto --bin gen` (it rewrites `schema/hennery-protocol.schema.json` and `web/src/generated/protocol.ts`).

- [ ] **Step 4: Run the tests to see them pass**

Run: `nix develop -c cargo test -p hennery-proto --locked --test codegen`
Expected: PASS, 9 tests.

- [ ] **Step 5: Revert-probes**

| Line | Change | Fails |
|---|---|---|
| `render_ts`'s `TS::docs()` | removed | `a_types_doc_reaches_the_typescript` |
| `McpCredentialRequest.token`'s doc | removed | `the_gateways_wire_types_document_every_field_and_variant` |
| `McpCredKind::OauthDcr`'s doc | removed | the same |
| `wrong_cred_kind` in `McpCredentialRequest`'s doc | removed | `a_types_doc_reaches_the_typescript` |

Each also fails `generated_*_matches_the_checked_in_copy`, which says only that the files are stale.

- [ ] **Step 6: The five checks, then commit**

```bash
git add crates/hennery-proto schema web/src/generated
git diff --cached --stat
git -c commit.gpgsign=false commit -m "docs(gateway): document the gateway's wire types, routes and error codes, into the TypeScript"
```

(The tests first, as `test(gateway): the gateway's wire types document their fields, routes and error codes in the TypeScript`.)

---

## The security review's answers

Two security reviews ran on the maintainer's behalf (opus, 2026-10-02), on the plan and the scratch diff. Both answered **approve after amendments**. Their findings, and what was done:

| Finding | Kind | Done |
|---|---|---|
| The key version is bound in the AAD but no test proves it (R1) | Required | `crypto::tests::the_key_version_is_bound_into_the_aad`, revert-probed (decision 7) |
| `static_credential` returns the token without where it goes, so a reader could race an edit (R2) | Required | `StaticCredential`, one statement (decision 14) |
| A set but unreadable key source is skipped and a key file made instead (R3, both reviews) | Required | non-UTF-8 `HENNERY_MASTER_KEY` and a failing credential look-up are errors; a warning when `$CREDENTIALS_DIRECTORY` holds no key (decision 6) |
| A lost or wrong key stops the whole collector, with no recovery the operator can run (finding 1) | Required | every such error names the way out; `KeyUnavailable` separates the case; decision 8 says only the newest row is checked; the alternative is written up as Q1 |
| Sync the directory after making `master.key` (O2) | Optional | taken (decision 6) |
| Cap the mounts request (O3) | Optional | taken: 1024 hosts, 64-byte ids, before the transaction (decision 13) |
| Name the AAD's field by table (finding 5) | Optional | taken: `gw_credentials.static_token` (decision 7) |
| Strip the key variables in the CLI tests; test the environment source end to end (O4) | Optional | taken: `offline()` removes both; `a_master_key_from_the_environment_is_used_and_never_written_down` |
| Check `master.key`'s owner and links (O1) | Optional | links taken (`nlink == 1`); the owner check declined (decision 6) |
| Bind the origin and owner into the AAD (O5) | Optional | declined (decision 7) |
| `Cache-Control: no-store` (O6) | Optional | taken (decision 20) |
| `BEGIN IMMEDIATE` (O7) | Optional | already so: `db::configure` makes every transaction `IMMEDIATE` |
| Zeroizing gaps in `static_credential` (O8) | Optional | taken (decision 14); the request body's `String` recorded, not wiped |
| More reserved headers (N6) | Note | taken (decision 13) |
| A stale step-up against an unknown id is not tested (N7) | Note | added to `where_a_token_can_go_changes_only_with_a_fresh_step_up` |
| 8d: one read for the token and where it goes; drop the agent's own `Authorization` | Note | "After this plan" |
| `has_ciphertext`/`check_key` cover `gw_credentials` only; `check_key` refuses non-static kinds | Note | "After this plan" (8f, stdio) |
| The purge race (a connection created between the hook and the hat's delete) | Note | "After this plan" (purge) |
| Delete mounts in `on_host_revoked` (N3) | Note | decision 12, "After this plan" (8d) |
| A `null` for a non-clearable field reads as absent | Note | recorded (decision 11) |
| The key stays in the collector's environment; `docker run -e` shows it | Note | decision 6; docs in 8g |
| Spec write-backs (decisions 1, 3, 4) | Note | "Spec amendments" |
| `up`'s multi-hat warning (kernel §10) is deferred to 8g; 8g must land before a release | Note | "After this plan" |

**Product questions** the specs and the maintainer's table do not answer, put to the gateway lane parent (2026-10-02), with their defaults:
- Q1, what a lost or wrong key takes down: **open for the maintainer, default chosen, reversible** — refuse to start (decision 8, with the trade-off against serving with the gateway off).
- Q2, a secret inside a vendor's URL: **open for the maintainer, default chosen, reversible** — documented, never logged whole (decisions 9 and 19); to be answered before 8d.
- Q3, a `DELETE …/credential` route: **confirmed by the lane parent** — none (decision 14).
- Q4, step-up on deleting a connection: **confirmed by the lane parent** — needed (decision 4).
- Q5, binding the origin and the owner into the AAD (the re-confirmation's finding 2): **declined by the security review; confirmed by the lane parent** (2026-10-02) — not bound (decision 7), reversible by a re-seal; not a maintainer question, and 8d does not wait on it.

Decision 19 (an upstream URL is shown only as its origin) came from the fleet parent, through the lane (L11), after the reviews.

**The scoped re-confirmation, round 1** (a fresh opus security reviewer, 2026-10-02, on the scratch diff before these amendments, the plan and these answers, with L11 and Task 5): **not confirmed**. It confirmed R1, R2, R3 and finding 1 closed, each by its test that fails without it; L11's log lines and error messages; step-up, the owner filter and the key kept from the host and agents; and the documented codes against the code. Its findings, and what was done:

| Finding | Kind | Done |
|---|---|---|
| 1. `rest::url_origin` shows raw input it does not parse (a `/` inside the user info, text before `://`, a `\`) | Required | it parses with `url::Url` and fails closed; the inputs named, and a 422 wrong-type body, are in `an_upstream_url_is_logged_and_shown_only_as_its_origin`; revert-probed (decision 19) |
| 2. O5 was declined for the wrong reason | Optional, product question | decision 7 rewritten with the trade-off; Q5 open for the maintainer |
| 3. Some key errors name no way out; `from_env`'s miss `KeyUnavailable` | Optional | recorded (decision 8); "After this plan" (8g) |
| 4. `no-store` missing on the session's and the browser rules' refusals | Optional | taken: the layer moved outside `operator_only`; `answers_are_never_cached` covers a 401 and a 403; revert-probed (decision 20) |
| 5. 405 is not documented | Note | documented (Task 5) |
| 6. `CREDENTIALS_DIRECTORY` reaches the agents under `up` | Note | "After this plan" (8g) |
| 7. The give-up text says `<data>/hennery.db`; under `up` it is `<data>/collector/hennery.db` | Note | "After this plan" (8g) |
| Decision 6 leaned on a 0700 data directory that is only warned about | Correction | decision 6 corrected |

After round 1, two rulings came through the lane: the operator's on `http` (decision 9: no code change, one API test) and the fleet parent's on the error body (decision 15: one test, revert-probed).

**The scoped re-confirmation, round 2** (another fresh opus security reviewer, 2026-10-02, on the round-1 fixes and the two rulings, and the whole diff): **confirmed with notes**, nothing required. It confirmed round 1's finding 1 closed (`url_origin` fails closed on every `Debug` that shows a URL, and no 8a path shows more than L11 allows), the `no-store` move without a change in the order of the browser rules, the session, the body limit and step-up, decisions 6, 8, 9 and 15 against the code, Q5's framing as honest with "not bound" an acceptable interim default (no token leaves the process before 8d), and Task 5's codes against the code. Its notes, and what was done:

| Note | Done |
|---|---|
| The 422 case's canary sat in a list, which serde never quotes | taken: the canary is a string in a boolean field; `update`'s `ApiJson` revert-probed against it |
| Q5's "reversible until credentials exist outside tests" is loose: 8a stores real tokens once merged | reworded: reversible by a re-seal (decision 7) |
| The probe log names each probe and its outcome, not its change | the Step 5 tables name each change |
| The kernel's own refusals have the `{code, message}` shape by reading `auth_api::error`, not by a test | recorded: the shared `ApiError` with `session_id` skipped when absent |

**Who confirmed what:** the two security reviews and both rounds of the re-confirmation were opus subagents acting on the maintainer's behalf; Q3 and Q4 were confirmed by the gateway lane parent; decision 9's `http` ruling is the operator's, decision 15's error body and decision 19 (L11) the fleet parent's, decision 21 the lane parent's; Q5 declined by the security review and confirmed by the lane parent; Q1 and Q2 are open for the maintainer, each with its default in place.

## After this plan

**What later sub-plans inherit:**
- **8b-ii (egress, merged as #75 before 8a):** under `InternalNetwork`, `check_url` sends plain `http` to internal addresses only (L12), which is what 8a's `internal_network` saves. Connections are saved with any public or private address; egress refuses non-public ones at request time unless the connection is internal (decision 9, L7). Left for 8d: `http` to loopback on an unmarked connection, which 8a refuses when saving and egress would send (decision 9; the lane parent's ruling: 8a stays as reviewed).
- **8d (proxy):**
  - find a connection by the token's owner and the slug (`UNIQUE (owner_id, slug)`); take its token **and** where it goes from `GatewayStore::static_credential(id, &key)` alone (`StaticCredential`, one statement), never the URL from a second read (decision 14);
  - when `static_header` is not `Authorization`, drop the agent's own `Authorization` (its session token) before forwarding, with a test (gateway §5.2);
  - log an upstream only as `url_for_logs` gives it (decision 19);
  - implement `on_host_revoked` for the gateway and delete the host's mounts there (decision 12), before any plan deletes host rows;
  - send it as `<static_header>: <static_prefix><token>`; the header is never one the proxy sets or filters (decision 13);
  - a mount counts only while its host is not revoked: join `hosts` on `revoked_at IS NULL`, as `SELECT_MOUNTS` does (decision 12);
  - apply `tool_allowlist` (`None`: every tool; `[]`: none).
- **8e (sessions wiring):** the gateway's functions taking `&rusqlite::Transaction` (L1) go in `store.rs`, where the audit reads them; `GatewayStore` keeps its own connection.
- **8f (OAuth):**
  - lift `CredKind::is_supported` for the two OAuth kinds, and give each its AAD field in `crypto.rs`, named by table (`gw_credentials.oauth_tokens`, `gw_oauth_clients.client_secret`; the static token is `gw_credentials.static_token`); `check_key` refuses any kind but `static` today and must open each kind with its field;
  - `update`'s §4.6 branch deletes `gw_oauth_clients` too, beside the credential, in the same transaction;
  - `has_ciphertext` counts `gw_oauth_clients.client_secret_ciphertext` too, and so must the stdio servers' plan count `gw_stdio_servers.env_ciphertext`;
  - the probe sets `status`, `status_note`, `account_label` and `status_at`.
- **8g (standalone, renderers, `rotate-key`):** `rotate-key` re-seals every row under a new `key_version` (`KEY_VERSION` is 1 now); a key per version, and `has_ciphertext`/`check_key` across owners, are its to decide (decision 16). `up`'s multi-hat warning reads `gw_credentials` joined to `gw_connections.hat_id`; kernel §10 wants it as soon as credentials can be stored, so 8g lands before a release. Its docs say `docker run -e HENNERY_MASTER_KEY` shows the key in `docker inspect`. If the maintainer answers Q1 the other way, `run_collector` matches `KeyUnavailable` (decision 8), and `from_env`'s errors join it first. From the re-confirmation: the remaining key errors name the way out (a `master.key` of the wrong length, a credential that does not read); `CREDENTIALS_DIRECTORY` joins `HOST_SECRET_VARS` when the service docs cover systemd credentials; the give-up text names the database's real path (`<data>/collector/hennery.db` under `up`).
- **Purge (plan 9c):** call `GatewayStore::purge_hat(hat_id)` from `on_hat_purged`, before the kernel deletes the hat row: the foreign key refuses the delete while the hat has connections. A frozen hat (`purged_hats`) should be refused by the gateway's create too, and a connection created between the hook and the hat's delete makes that delete fail on the foreign key: retry the hook (it is idempotent), or refuse a create on a frozen hat. Whoever merges second adds the check beside the hat's.
- **Frontend (plan 4):** the MCP view, on the documented types (decision 21: each request type's doc names its route's answer and codes); on 403 `step_up_required` step up and retry; `has_credential` and "applies to new and resumed sessions"; the slug and the hat are fixed once created.

- **Capabilities (L14):** `crates/hennery-kernel/src/capabilities.rs` is not on `main` at `522e802`. Whoever merges second of 8a and frontend 4b adds `mcp_connections` to `features()`, with a test that `GET /api/capabilities` lists it.
- **Follow-ups from the execution's reviews** (minor, none blocking):
  - `model::url_for_logs` and `rest::url_origin` are the same function in two crates; make one call the other.
  - `api.rs` maps a status string it does not know to `NotConnected` silently; 8f, adding a status, should make the model hold an enum, or log the fallback.
  - The incoming token is a plain `String` until it is sealed (the review's O8, recorded); `Zeroizing<String>` in the handler would wipe it.
  - `store.rs` echoes `hat_id` with no bound (`no hat …`); bound it before the transaction as host ids are, and do not echo it.
  - `model::label_problem`'s message still says "printable characters"; it counts bytes.
  - `render_ts` does not escape `*/` in a doc; a codegen test that no description contains it would guard `protocol.ts`.
  - Untested small guards: the directory sync after making `master.key`, the warning on an empty `$CREDENTIALS_DIRECTORY`, a credential that is a directory or FIFO, a `master.key` look-up error other than not-found, `StaticCredential`'s `Debug` of the URL.
  - The CLI's end-to-end test does not check that the merged router keeps the session and `no-store` (Task 3's router tests do).

**Operator items:** none for 8a. Live calls to vendors start with 8d (L10).

**Open for the maintainer:** Q1 and Q2 ("The security review's answers"); each default is in place and reversible.

**Spec amendments (written back with the execution record):**
- decision 1: gateway §2: slugs unique per owner.
- decision 3: gateway §2: list endpoints never read the credential tables' secret columns; whether a row exists is read by its key.
- decision 4: gateway §9 and kernel §3.4: `DELETE` needs step-up; `PATCH` needs it for the static header and prefix too; mounts need none. §9's table is written back as built: each route's answer, its step-up and its codes.
- decisions 6–8: gateway §6: the sources and their order, the AAD's encoding (lengths, the field named by table), a missing or wrong key stops the start with the way out (Q1 open).
- decision 19: gateway §5 and §11: an upstream URL is logged and shown only as its origin.
- decision 12: gateway §2: a revoked host's mounts are unused and unlisted.
- ACP core §1: "Built so far": the `hennery-gateway` crate exists (8a); no `SessionMcp` yet.

---

_Generated with Claude AI — please review before distribution._
